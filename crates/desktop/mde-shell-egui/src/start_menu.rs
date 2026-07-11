//! WIN7-2 — the **Start Menu** shell (design `docs/design/win7-desktop-survey.md`,
//! locks #2/#4/#10/#13/#14; WIN7-DESKTOP-1's second implementation unit).
//!
//! The fixed-size overlay panel that replaces the dock's Start-cell-opens-
//! Console behaviour (WIN7-1 relabelled the cell "Start" but left its click
//! wired to Console directly — this unit is the real two-pane Start Menu lock
//! #4 describes). It reuses the SAME floating-`egui::Area` + [`Motion`]-tweened
//! slide-up pattern `console.rs`'s old standalone panel and `dock.rs`'s
//! vertical dock both already use (not a new mechanism): fixed-size (lock #2 —
//! never full-screen, never resizable), anchored bottom-left beside the
//! vertical dock column (`x = DOCK_W`, the Console front door's existing
//! footprint), opening **upward** from the bottom edge.
//!
//! **Panes (lock #4):** left = a placeholder for WIN7-3's live-tile grid (this
//! unit ships the shell only, no tile content); right = [`console::ConsoleState`]
//! embedded via [`console::console_content`] — CONSOLE-1's existing operational
//! front door (groups, Power section, Custom entries, the CONSOLE-2 spawn-tab
//! seam), unchanged and fully working, not a bare label. Migrating Console's
//! *content* here is real work this unit does; *redesigning* its presentation
//! for the new home (lock #10) is WIN7-5's job — today it renders exactly as
//! it always has, just embedded at this panel's right-pane rect instead of
//! mounting its own independent `Area`.
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
//! **The VDOCK-1 / Super key overlap (a judgment call, not covered by the
//! survey):** the vertical dock (`dock.rs`) ALREADY binds a clean Super tap to
//! `DockState::toggle` (VDOCK-1, `docs/design/vertical-dock.md` lock #13) —
//! it's the shell's only surface launcher until WIN7-3 lands real tiles here,
//! so this unit must not steal Super away from it (that would strand every
//! surface behind an unpinned, now-unreachable dock). Lock #13 in the win7
//! survey just says "Super opens the Start Menu," without addressing that
//! pre-existing claim. Resolution: `main.rs` applies the SAME clean-Super-tap
//! drain (`HotkeyRouter::take_dock_toggle`) to BOTH `DockState::toggle` AND
//! `StartMenuState::toggle`, so one Super tap reveals both — not a conflict in
//! practice, since the Start Menu already mounts immediately beside the dock
//! column (`x = DOCK_W`), so revealing both together reads as "the whole nav
//! chrome" rather than two unrelated popups. This is a deliberate, flagged
//! choice, not a discovered fact — worth a confirm from the operator, and
//! likely moot once WIN7-3's tiles let the vertical dock retire.
//!
//! **Accesskit (lock #14):** the panel itself carries a role + label before any
//! content lands — `Role::Menu` for the whole panel, `Role::Group` landmarks
//! for each pane — so a screen reader can already navigate the shell. Deep
//! per-tile / per-row accesskit is WIN7-3's (tiles) and WIN7-7's (the full
//! sweep) job, not re-litigated here.
//!
//! **WIN7-3 update:** the left pane described above as an empty placeholder is
//! now the real live-tile grid (locks #6/#7/#8/#23): all 18 [`Surface::ALL`]
//! entries, grouped into lock #8's 7 function-based groups (Mesh Control ·
//! Desktop & Session · Media · Files & Data · Web & Tools · Comms · System —
//! [`TILE_GROUPS`]), each a uniform [`TILE_W`]×[`TILE_H`] tile (lock #6 — one
//! size, no variants). A tile wears the SAME glyph the app picker already
//! draws (`Surface::icon_id`) plus a NEW text label (`Surface::label`,
//! added this unit): the picker itself deliberately carries no per-icon
//! captions (`dock.rs`'s own PICKER-1 lock), so there was no existing label
//! table to inherit, only the icon one. A click reuses the picker's own
//! click-vs-Enter/Space activation predicate (`dock::response_activated`,
//! widened to `pub(crate)` for this reuse, not reimplemented) and records
//! the surface in a new [`StartMenuState::tile_activation`] slot, drained by
//! `main.rs` exactly like an embedded Console `Goto` request — both panes
//! end in the same "go to this surface, close the whole menu" outcome (lock
//! #23), just raised from different data. [`LEFT_PANE_W`] is no longer
//! WIN7-2's arbitrary 288pt placeholder; it is now sized to the real grid
//! this unit renders. Static content only (lock #5's live-fact rotation is
//! WIN7-4's job) — this unit leaves a [`tile_status_tint`] seam WIN7-4 can
//! light up rather than hardcoding "never any live data."
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
//! **WIN7-5 update:** the right pane's content is no longer rendered
//! "exactly as it always has" — the paragraph above describing that as
//! WIN7-5's still-open job is now superseded; see `console.rs`'s own module
//! doc for the redesign itself (its presentation, not this module's). This
//! module's OWN embedding contract is unchanged by that redesign: the same
//! [`console::console_content`] call at the same right-pane rect, the same
//! [`ConsoleState::set_open`] mirror, the same self-closure propagation —
//! WIN7-5 changed what `console_content` draws inside the rect this module
//! hands it, not the seam between the two modules.
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
//! search-box spot); it auto-focuses when the menu opens so "open, then just
//! type" filters live. An empty query is the unchanged grouped grid (zero
//! behaviour change); the moment anything is typed, [`search_matches`] ranks
//! the 18 tileable surfaces (case-insensitive: a label prefix beats a label
//! substring beats a group-name hit) and [`search_results`] paints that flat
//! list in place of the grid — Up/Down move a highlight, Enter launches (the top
//! match by default), a row click launches that row, Esc clears the query
//! (a second, now-empty Esc dismisses the menu). Every launch routes through
//! the SAME [`StartMenuState::tile_activation`] seam a tile click already uses,
//! so `main.rs`'s existing drain carries a searched launch with NO new
//! plumbing (this whole feature is self-contained to `start_menu.rs`). The
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

use std::time::Duration;

use mde_egui::egui;
use mde_egui::{Motion, Style};
use mde_lighthouse_health::LighthouseHealth;

use crate::chrome::MeshSummary;
use crate::console::{self, ConsoleState};
use crate::dock::{icon_texture, response_activated, Surface, DOCK_W};
use crate::status::{self, StatusSegments};

// ── geometry ─────────────────────────────────────────────────────────────────

/// The stable id of the Start Menu's floating [`egui::Area`] layer.
const START_MENU_AREA: &str = "start-menu-area";

/// The egui memory key for the panel's slide animation (the `console.rs`
/// `SLIDE_KEY` / `dock.rs` `DOCK_SLIDE_KEY` idiom, restated here since the
/// slide/Area machinery now lives in this module instead).
const SLIDE_KEY: &str = "start-menu-slide";

/// A 1px hairline rule (the dock's/console's `HAIRLINE_W` restated —
/// module-private in each, the established per-module idiom).
const HAIRLINE_W: f32 = 1.0;

// ── tile grid geometry (WIN7-3, locks #6/#7/#8) ─────────────────────────────

/// One tile's height — `SP_XL + SP_M` (48pt), the SAME cell-height
/// composition `dock.rs`'s own (module-private) `CELL_W` icon-cell token
/// already uses, restated here per this module's own established
/// per-file-restatement idiom (see [`HAIRLINE_W`] above). Every one of the
/// 18 tiles shares this ONE size (lock #6 — no small/wide/large variants).
const TILE_H: f32 = Style::SP_XL + Style::SP_M;

/// One tile's width — `SP_XL · 2.5` (80pt): wider than tall, so a full
/// surface label (e.g. "Infra as Code") has real room beside the shorter
/// ones. Every tile still shares this ONE width (lock #6 rules out
/// per-tile small/wide/large *variants*, not a non-square aspect ratio).
const TILE_W: f32 = Style::SP_XL * 2.5;

/// The gap between adjacent tiles, in both directions of the grid.
const TILE_GAP: f32 = Style::SP_XS;

/// How many tiles sit in one row before wrapping. The widest of lock #8's 7
/// groups (Mesh Control / Media / Files & Data / Web & Tools) has exactly 3
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
/// WIN7-2 shipped this pane at an arbitrary 288pt placeholder ("WIN7-3 will
/// very likely resize it" — its own doc comment); this is that resize,
/// derived from the real grid this unit renders rather than picked by eye.
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

/// The tile grid's total content height: 7 group headings + 7 single tile
/// rows (see [`TILE_COLUMNS`]'s note — every locked group fits one row
/// today) + 6 inter-group gaps. Comfortably inside [`PANEL_H`] minus its own
/// top/bottom [`PANE_PAD`] inset — pinned by a test below rather than
/// trusted by eye. `#[cfg(test)]`: nothing in the render path reads a
/// pre-summed total (`left_pane` accumulates `y` incrementally instead), so
/// this is verification-only data (the `status.rs` `local_grade_pip_id`
/// `#[cfg(test)]`-on-a-top-level-item idiom), not dead weight in a release
/// build.
#[cfg(test)]
const TILE_GRID_CONTENT_H: f32 = 7.0 * (GROUP_HEADING_H + TILE_H) + 6.0 * GROUP_GAP;

// ── type-to-launch search (SHELL-UX-3) ──────────────────────────────────────

/// The search field's row height — a single [`Style::SP_L`] (24pt) line that
/// tucks into the left pane's bottom headroom BELOW the 7-group tile grid
/// without overlapping the last tile row (the grid content bottoms out ~8pt
/// above this band; pinned by a test below rather than trusted by eye). Win7's
/// Start Menu puts its search box at exactly this spot — the bottom of the
/// left pane, under the app list.
const SEARCH_H: f32 = Style::SP_L;

/// One search-result row's height — a compact list row (leading icon · label ·
/// dim group name), `SP_L + SP_XS` (28pt). Sized so all 18 tileable surfaces
/// fit the results area at once even when a one-letter query matches every
/// one (18 · 28 = 504pt, inside the ~532pt results band with one row of
/// headroom to spare — pinned by a test below).
const RESULT_ROW_H: f32 = Style::SP_L + Style::SP_XS;

/// The search-result row's leading icon size — [`Style::SP_M`] (16px), smaller
/// than a tile's 24px [`TILE_ICON`] glyph because a result row is a list line,
/// not a tile face.
const RESULT_ICON: f32 = Style::SP_M;

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

/// One labelled group of the left pane's tile grid (lock #8). Mirrors
/// `dock.rs`'s `Group` / `console.rs`'s `ConsoleGroup` shape — this module's
/// own copy since the Start Menu's tile grouping is its own domain concern
/// (lock #8), distinct from the app picker's PICKER-1 grouping.
struct TileGroup {
    /// The group heading, painted by [`tile_group_heading`] (visually
    /// matching `console.rs`'s own `heading()` row, per this unit's steer to
    /// match Console's precedent since it sits right next to this pane).
    label: &'static str,
    /// The group's surfaces, kept in [`Surface::ALL`] relative order (the
    /// `dock.rs` `Group::surfaces` L7 convention) — lock #8's own listed
    /// order already satisfies this, checked by a test below.
    surfaces: &'static [Surface],
}

/// The 7 function-based groups in their locked order (lock #8), each listing
/// its surfaces in [`Surface::ALL`] relative order. Unlike the app picker's
/// `GROUPS` (which pulls the Workbench/System/Desktop out to standalone
/// cells), every one of the 18 [`Surface::ALL`] entries sits inside exactly
/// one group here — lock #8 places all 18, none outside. Drives the tile
/// render + the shell tests (the one grouping authority for this pane).
const TILE_GROUPS: [TileGroup; 7] = [
    TileGroup {
        label: "Mesh Control",
        surfaces: &[Surface::Workbench, Surface::MeshView, Surface::InfraCode],
    },
    TileGroup {
        label: "Desktop & Session",
        surfaces: &[Surface::Desktop],
    },
    TileGroup {
        label: "Media",
        surfaces: &[Surface::Music, Surface::Media],
    },
    TileGroup {
        label: "Files & Data",
        surfaces: &[Surface::Files, Surface::Bookmarks, Surface::Storage],
    },
    TileGroup {
        label: "Web & Tools",
        surfaces: &[Surface::Browser, Surface::Terminal, Surface::Editor],
    },
    TileGroup {
        label: "Comms",
        // Voice (SIP calling) is a communications surface, not a media player:
        // dock.rs `GROUPS` — the canonical taxonomy — groups it with Chat/Phones,
        // so this tile grouping matches (kept in `Surface::ALL` relative order).
        surfaces: &[Surface::Voice, Surface::Chat, Surface::Phones],
    },
    TileGroup {
        label: "System",
        // Explorer (discover every mesh/LAN/cloud unit) tiles here beside About —
        // whose body IS the Device-Manager hardware inventory — as the pane's
        // inventory/inspection pair. The canonical picker (`dock.rs` GROUPS) files
        // it under Mesh with the Mesh Map; the two taxonomies group per their own
        // concern (lock #8), so this pane's functional grouping keeps it with the
        // inventory surfaces rather than adding a fourth mesh tile that would wrap.
        surfaces: &[Surface::System, Surface::About, Surface::Explorer],
    },
];

// Compile-time guard: every `Surface::ALL` entry appears in `TILE_GROUPS`
// exactly once (the `dock.rs` `GROUPS` completeness-guard idiom, restated
// here since this table is its own domain concern — lock #8's grouping, not
// `dock.rs`'s picker grouping) — so a future `Surface` addition that forgets
// to place a tile fails the BUILD, not a silent missing/duplicate tile.
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

// ── state ────────────────────────────────────────────────────────────────────

/// The Start Menu's cross-frame state: the open latch (driven by the Start
/// cell click and the Super key, lock #13), the same-frame click-away guard
/// (the Console/VDOCK-4 `just_toggled` idiom, restated here since this panel
/// now owns its own outer `Area`/dismiss machinery), and (WIN7-3) a pending
/// tile-click surface activation. Pure (no egui handles), so open/close and
/// tile activation are unit-tested without a GPU. WIN7-4 (tile rotation) and
/// WIN7-8 (multi-seat sync) are what grow this further.
#[derive(Debug, Default)]
pub struct StartMenuState {
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
}

impl StartMenuState {
    /// Whether the panel is up.
    pub(crate) const fn is_open(&self) -> bool {
        self.open
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
/// sliding up from the bottom edge, anchored beside the vertical dock column
/// (lock #4's bottom-left footprint, the Console front door's former spot).
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
        .fixed_pos(egui::pos2(DOCK_W, top))
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
            let search_ate_escape = start_menu_search(ui, left_rect, state);
            console::console_content(ui, right_rect, console);
            search_ate_escape
        });
    let search_ate_escape = area.inner;

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
    // Console/VDOCK-4 `just_toggled` guard).
    if state.open && !state.just_toggled && area.response.clicked_elsewhere() {
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
    if state.open && t >= 0.999 {
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
    painter.rect_filled(rect, Style::RADIUS, Style::SURFACE);
    painter.rect_stroke(
        rect,
        Style::RADIUS,
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
        egui::StrokeKind::Inside,
    );
    painter.vline(
        rect.left() + LEFT_PANE_W,
        (rect.top() + Style::SP_XS)..=(rect.bottom() - Style::SP_XS),
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
    );
}

/// The left pane (WIN7-3, locks #6/#7/#8; WIN7-4, lock #5): [`TILE_GROUPS`]'
/// 7 headed sections, each a row of uniform [`TILE_W`]×[`TILE_H`] tiles in
/// [`Surface::ALL`] order. Returns the clicked tile's surface, if any, for
/// the caller to route + close the whole panel with (lock #23 — a single
/// click activates, mirroring the embedded Console pane's own
/// click-routes-and-closes behaviour). Reads the ONE frame clock
/// (`ui.input(|i| i.time)`) once and threads it to every tile, so every
/// rotating tile in the grid advances in lockstep off the SAME clock rather
/// than each reading its own slightly-different sample.
#[allow(
    clippy::cast_precision_loss, // row/col indices are tiny (< TILE_COLUMNS)
    clippy::suboptimal_flops     // layout arithmetic reads clearer than mul_add
)]
fn left_pane(ui: &egui::Ui, rect: egui::Rect, inputs: &TileFactInputs) -> Option<Surface> {
    let time_secs = ui.input(|i| i.time);
    install_tiles_live_summary(ui.ctx(), rect, inputs, time_secs);
    let mut activated = None;
    let x0 = rect.left() + PANE_PAD;
    let mut y = rect.top() + PANE_PAD;
    for group in &TILE_GROUPS {
        let heading_rect = egui::Rect::from_min_size(
            egui::pos2(x0, y),
            egui::vec2((rect.width() - PANE_PAD * 2.0).max(0.0), GROUP_HEADING_H),
        );
        tile_group_heading(ui, heading_rect, group.label);
        y += GROUP_HEADING_H;

        for (i, &surface) in group.surfaces.iter().enumerate() {
            let col = (i % TILE_COLUMNS) as f32;
            let row = (i / TILE_COLUMNS) as f32;
            let tile_rect = egui::Rect::from_min_size(
                egui::pos2(
                    x0 + col * (TILE_W + TILE_GAP),
                    y + row * (TILE_H + TILE_GAP),
                ),
                egui::vec2(TILE_W, TILE_H),
            );
            let facts = tile_facts(surface, inputs);
            let tint = tile_status_tint(surface, inputs);
            if tile(ui, surface, tile_rect, &facts, tint, time_secs) {
                activated = Some(surface);
            }
        }
        let rows = group.surfaces.len().div_ceil(TILE_COLUMNS).max(1);
        y += rows as f32 * (TILE_H + TILE_GAP) - TILE_GAP + GROUP_GAP;
    }
    activated
}

/// One tile-group heading — visually matches `console.rs`'s own
/// (module-private) `heading()` row exactly (same uppercased micro-label,
/// same `SMALL`/`TEXT_DIM` treatment, same `SP_XS` left inset), restated
/// here since it's private to that module and this pane paints via explicit
/// rects rather than `console.rs`'s layout-managed `ui.allocate_exact_size`
/// (this module's own established direct-painter style, e.g. [`paint_frame`]).
fn tile_group_heading(ui: &egui::Ui, rect: egui::Rect, label: &str) {
    ui.painter().text(
        egui::pos2(rect.left() + Style::SP_XS, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label.to_uppercase(),
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT_DIM,
    );
}

/// One live tile (WIN7-3, locks #6/#8/#23; WIN7-4, lock #5): a uniform
/// [`TILE_W`]×[`TILE_H`] cell wearing the surface's existing picker glyph
/// (`Surface::icon_id`, the SAME [`icon_texture`] loader + the SAME 24px
/// [`TILE_ICON`] size the app picker's own cells use) over its label slot,
/// which now shows [`tile_display_text`]'s fold of `facts` instead of always
/// `Surface::label()` (WIN7-3's own static behaviour, still exactly what
/// renders when `facts` is empty). A hover brightens both the fill and the
/// tint — the same two-tone contract the app picker's own cells already use
/// (§4, one hover language, not a second one invented here). A click (or
/// Enter/Space while focused — [`response_activated`], reused verbatim
/// rather than reimplemented) returns `true` so [`left_pane`] can route +
/// close the whole panel (lock #23). Exports its own accesskit `Button` node
/// (lock #14) carrying the CURRENT display text as its value — not
/// individually `Live::Polite` (see [`install_tile_accessibility`]'s own doc
/// for why); [`install_tiles_live_summary`] is the grid's one live announcer.
fn tile(
    ui: &egui::Ui,
    surface: Surface,
    rect: egui::Rect,
    facts: &[String],
    status_tint: Option<egui::Color32>,
    time_secs: f64,
) -> bool {
    let resp = ui.interact(rect, tile_id(surface), egui::Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter().clone();

    if hovered {
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    }
    let tint = if hovered {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };

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
    let display_text = tile_display_text(surface, facts, time_secs);
    painter.with_clip_rect(rect).text(
        egui::pos2(rect.center().x, rect.bottom() - Style::SP_XS),
        egui::Align2::CENTER_BOTTOM,
        display_text,
        egui::FontId::proportional(Style::SMALL),
        tint,
    );

    install_tile_accessibility(ui.ctx(), surface, rect, display_text);
    response_activated(ui, &resp)
}

/// The stable id of one tile's interactive rect (the `dock.rs` `pick_cell_id`
/// idiom restated — tests read a tile's settled `Rect` back to click its
/// exact centre, the addressable-cell idiom).
fn tile_id(surface: Surface) -> egui::Id {
    egui::Id::new(("start-menu-tile", surface))
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
#[allow(clippy::too_many_lines)] // one match arm per Surface::ALL variant (18), same shape as badge_for
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
        Surface::Editor | Surface::About | Surface::Explorer => Vec::new(),
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
/// everywhere else — most of the 18 surfaces are plain counts with no
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

/// Which of `len` facts should show right now, given the current frame clock
/// (`ui.input(|i| i.time)`, seconds since the egui `Context` was created —
/// the SAME time source `explorer.rs`'s `hero_card`/`tick_ambient` already
/// read for their own time-driven visuals, not a new mechanism). Pure: no
/// per-tile stored state, so [`StartMenuState`] stays the "no egui handles"
/// pure struct its own doc comment promises — every tile derives its
/// current step fresh from the ONE shared clock each frame, so they all
/// advance in lockstep and the displayed value never drifts/accumulates
/// error the way a stored "last rotated at" counter could. `len <= 1` always
/// answers `0` (nothing to rotate between).
#[allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "time_secs is always >= 0.0 (a monotonic clock since context creation); \
              truncating to whole rotation ticks is the intended behaviour"
)]
fn rotating_fact_index(len: usize, time_secs: f64) -> usize {
    if len <= 1 {
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
fn tile_display_text<'a>(surface: Surface, facts: &'a [String], time_secs: f64) -> &'a str {
    match facts.len() {
        0 => surface.label(),
        1 => facts[0].as_str(),
        len => facts[rotating_fact_index(len, time_secs)].as_str(),
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
/// Returns whether the search consumed this frame's Esc (query non-empty →
/// Esc cleared it rather than dismissing the menu); the caller uses that to
/// suppress the one-frame Console self-closure the shared Esc would otherwise
/// propagate (see [`start_menu_panel`]'s guard).
#[allow(clippy::suboptimal_flops)] // layout arithmetic reads clearer than mul_add (the left_pane idiom)
fn start_menu_search(ui: &mut egui::Ui, left_rect: egui::Rect, state: &mut StartMenuState) -> bool {
    // The search field sits in the left pane's bottom headroom; the grid /
    // result list get everything above it.
    let search_rect = egui::Rect::from_min_size(
        egui::pos2(
            left_rect.left() + PANE_PAD,
            left_rect.bottom() - PANE_PAD - SEARCH_H,
        ),
        egui::vec2((left_rect.width() - PANE_PAD * 2.0).max(0.0), SEARCH_H),
    );
    let content_rect = egui::Rect::from_min_max(
        left_rect.min,
        egui::pos2(left_rect.right(), search_rect.top() - Style::SP_XS),
    );

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
        // Empty query — the unchanged full grouped grid (WIN7-3 behaviour, no
        // change), and Esc dismisses the whole menu exactly as before.
        if let Some(surface) = left_pane(ui, left_rect, &state.tile_inputs) {
            state.tile_activation = Some(surface);
            state.close();
        }
        if state.open && esc_pressed(ui) {
            state.close();
        }
        return false;
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
    let clicked = search_results(ui, content_rect, &matches, state.search_highlight);

    // Enter launches the highlighted match (the top match by default); a click
    // on any row launches that one — both via the tile_activation seam.
    let launch = clicked.or_else(|| {
        (enter && !matches.is_empty())
            .then(|| matches[state.search_highlight.min(matches.len() - 1)])
    });
    if let Some(surface) = launch {
        state.tile_activation = Some(surface);
        state.close();
        return false;
    }
    if esc {
        // Esc over a live query clears it (and re-arms the emptied box's focus
        // so typing continues), never closing the menu — a second, now-empty
        // Esc is what dismisses it. Report it so the caller suppresses the
        // Console self-closure the shared Esc briefly triggered.
        state.clear_search();
        state.search_focus_pending = true;
        return true;
    }
    false
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
    let resp = ui.put(
        rect,
        egui::TextEdit::singleline(query)
            .hint_text("Search apps…")
            .font(egui::FontId::proportional(Style::BODY))
            .desired_width(rect.width())
            .return_key(None),
    );
    if *focus_pending {
        resp.request_focus();
        *focus_pending = false;
    }
    resp.changed()
}

/// Rank the 18 tileable [`Surface::ALL`] entries against `query`
/// (case-insensitive), best match first: a label *prefix* hit (rank 0)
/// outranks a label *substring* hit (rank 1), which outranks a hit on the
/// surface's *group name* only (rank 2 — so typing a category like "media"
/// still surfaces its members); ties keep [`Surface::ALL`] order, so the
/// ranking is stable and predictable, never RNG. A surface that matches
/// nowhere is dropped; an empty/whitespace query yields no matches (the caller
/// then shows the full grouped grid instead). Pure over the static surface
/// tables — unit-tested without a GPU, and the ONE authority the render +
/// keyboard both read so the painted list and Enter's target can't diverge.
fn search_matches(query: &str) -> Vec<Surface> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(u8, usize, Surface)> = Vec::new();
    for (idx, surface) in Surface::ALL.iter().copied().enumerate() {
        let label = surface.label().to_lowercase();
        let rank = if label.starts_with(&q) {
            0
        } else if label.contains(&q) {
            1
        } else if tile_group_label(surface).to_lowercase().contains(&q) {
            2
        } else {
            continue;
        };
        scored.push((rank, idx, surface));
    }
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, s)| s).collect()
}

/// Which [`TILE_GROUPS`] group a surface sits in (its label). Every
/// [`Surface::ALL`] entry sits in exactly one group (the compile-time guard
/// above), so the lookup always resolves for a searchable surface; the
/// never-searched `Timers` (outside `ALL`) would fall to `""`.
fn tile_group_label(surface: Surface) -> &'static str {
    TILE_GROUPS
        .iter()
        .find(|g| g.surfaces.contains(&surface))
        .map_or("", |g| g.label)
}

/// The ranked result list that replaces the grouped grid while a query is live
/// (SHELL-UX-3): one compact row per match — leading glyph, the surface label,
/// and its dim group name right-aligned — with the keyboard-highlighted row
/// (and any hover) wearing the SAME `SURFACE_HI` fill the tiles use, plus an
/// accent left stripe on the highlight so the Enter target reads at a glance.
/// A click on a row returns its surface for the caller to launch (via the
/// `tile_activation` seam), matching the tile grid's own click contract. An
/// empty result set paints an honest "no match" note (§7 — never a silent
/// blank). Direct-painter + `ui.interact` per row, the SAME addressable-cell
/// style [`tile`] uses (so a test can read a row's rect back by id and click
/// its centre).
#[allow(clippy::suboptimal_flops)] // layout arithmetic reads clearer than mul_add (the tile() idiom)
fn search_results(
    ui: &egui::Ui,
    rect: egui::Rect,
    matches: &[Surface],
    selected: usize,
) -> Option<Surface> {
    let painter = ui.painter().clone();
    if matches.is_empty() {
        painter.text(
            egui::pos2(rect.left() + PANE_PAD, rect.top() + PANE_PAD),
            egui::Align2::LEFT_TOP,
            "No apps match your search",
            egui::FontId::proportional(Style::BODY),
            Style::TEXT_DIM,
        );
        return None;
    }
    let mut activated = None;
    let x0 = rect.left() + PANE_PAD;
    let w = (rect.width() - PANE_PAD * 2.0).max(0.0);
    let mut y = rect.top() + PANE_PAD;
    for (i, &surface) in matches.iter().enumerate() {
        let row = egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(w, RESULT_ROW_H));
        let resp = ui.interact(row, search_result_id(surface), egui::Sense::click());
        let is_sel = i == selected;
        let hovered = resp.hovered();
        if is_sel || hovered {
            painter.rect_filled(row, Style::RADIUS, Style::SURFACE_HI);
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
        if let Some(tex) = icon_texture(ui.ctx(), surface.icon_id(), RESULT_ICON, tint) {
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
            surface.label(),
            egui::FontId::proportional(Style::BODY),
            tint,
        );
        painter.text(
            egui::pos2(row.right() - Style::SP_S, row.center().y),
            egui::Align2::RIGHT_CENTER,
            tile_group_label(surface),
            egui::FontId::proportional(Style::SMALL),
            Style::TEXT_DIM,
        );
        install_result_accessibility(ui.ctx(), surface, row, is_sel);
        if response_activated(ui, &resp) {
            activated = Some(surface);
        }
        y += RESULT_ROW_H;
    }
    activated
}

/// The stable id of one search-result row's interactive rect (the `tile_id`
/// addressable-cell idiom, restated for the result list so tests can read a
/// row's settled `Rect` back to click its centre).
fn search_result_id(surface: Surface) -> egui::Id {
    egui::Id::new(("start-menu-search-result", surface))
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

/// Install the panel-level accesskit tree (lock #14 — "the panel itself needs
/// proper roles/labels even before its content is filled in"): the whole
/// panel as a `Menu`, and each pane as a landmark `Group`, so a screen reader
/// can already navigate the shell before WIN7-5/7 land Console's own per-row
/// accesskit / the full sweep (the `status.rs` `install_status_accessibility`
/// idiom, restated here since this crate's dock/console panels have none
/// yet). WIN7-3 lands the tiles' own per-tile accesskit separately —
/// [`install_tile_accessibility`], called per tile from [`tile`].
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

/// The stable accesskit node id for one tile (WIN7-3, lock #14).
fn tile_accesskit_id(surface: Surface) -> egui::Id {
    egui::Id::new(("start-menu-tile-accesskit", surface))
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
/// "{Segment} status"). Deliberately NOT individually `Live::Polite`: a
/// screen reader hearing every one of up to 18 tiles announce itself on the
/// same rotation clock would be a spam regression, not an accessibility win
/// — [`install_tiles_live_summary`] is the ONE live-announcing node for the
/// whole grid, mirroring NOTIF-11's own shape exactly (one live
/// `status_live_region_id` summary + per-item value-bearing-but-not-live
/// `segment_pip` nodes in `status.rs`).
fn install_tile_accessibility(
    ctx: &egui::Context,
    surface: Surface,
    rect: egui::Rect,
    display_text: &str,
) {
    let _ = ctx.accesskit_node_builder(tile_accesskit_id(surface), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(surface.label());
        node.set_value(display_text);
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
fn tiles_live_summary(inputs: &TileFactInputs, time_secs: f64) -> String {
    let mut parts = Vec::new();
    for surface in Surface::ALL {
        let facts = tile_facts(surface, inputs);
        if facts.len() >= 2 {
            let text = &facts[rotating_fact_index(facts.len(), time_secs)];
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
) {
    let summary = tiles_live_summary(inputs, time_secs);
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

/// The stable accesskit node id for one search-result row.
fn search_result_accesskit_id(surface: Surface) -> egui::Id {
    egui::Id::new(("start-menu-search-result-accesskit", surface))
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
    surface: Surface,
    rect: egui::Rect,
    selected: bool,
) {
    let _ = ctx.accesskit_node_builder(search_result_accesskit_id(surface), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(surface.label());
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
    matches: &[Surface],
    selected: usize,
) {
    let value = if matches.is_empty() {
        format!("No apps match {query}")
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
    use super::{start_menu_panel, StartMenuState, DOCK_W, PANEL_H, PANEL_W};
    use crate::console::{self, ConsoleState};
    use crate::dock::Surface;
    use crate::status::{SegmentRollup, StatusSegments};
    use mde_egui::egui;
    use mde_egui::Style;

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
        let inside = egui::pos2(DOCK_W + 10.0, SZ.y - 10.0);
        assert_ne!(
            ctx.layer_id_at(inside),
            Some(start_menu_layer()),
            "a CLOSED Start Menu must not float an intercepting layer"
        );

        // Open on a fresh context (the slide latch settles at the open
        // endpoint on first sight, the console.rs precedent) -> claims exactly
        // its bottom-left footprint, anchored beside the dock column.
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
        // bottom-left footprint) and the strip LEFT of the dock column (the
        // panel sits BESIDE the dock, never under/over it) both stay
        // unclaimed while the Start Menu is open.
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
        let left_of_dock = egui::pos2(DOCK_W - 10.0, SZ.y - 10.0);
        assert_ne!(
            ctx.layer_id_at(left_of_dock),
            Some(start_menu_layer()),
            "the panel is anchored beside the dock column, not under it"
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

        let inside_the_reserved_band = egui::pos2(DOCK_W + 10.0, SZ.y - rail_h / 2.0);
        assert_ne!(
            ctx.layer_id_at(inside_the_reserved_band),
            Some(start_menu_layer()),
            "the Start Menu must not extend into the reserved taskbar band \
             (the last {rail_h}pt above the screen's true bottom edge)"
        );
        let just_above_the_band = egui::pos2(DOCK_W + 10.0, SZ.y - rail_h - 10.0);
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
    fn the_18_surfaces_are_grouped_into_lock_8s_7_function_based_groups() {
        // Lock #8's exact taxonomy + order — the `dock.rs`
        // `the_locked_group_taxonomy_and_order` precedent, restated for this
        // pane's own (different) grouping table.
        use Surface::{
            About, Bookmarks, Browser, Chat, Desktop, Editor, Explorer, Files, InfraCode, Media,
            MeshView, Music, Phones, Storage, System, Terminal, Voice, Workbench,
        };
        let expect: [(&str, &[Surface]); 7] = [
            ("Mesh Control", &[Workbench, MeshView, InfraCode]),
            ("Desktop & Session", &[Desktop]),
            ("Media", &[Music, Media]),
            ("Files & Data", &[Files, Bookmarks, Storage]),
            ("Web & Tools", &[Browser, Terminal, Editor]),
            ("Comms", &[Voice, Chat, Phones]),
            ("System", &[System, About, Explorer]),
        ];
        assert_eq!(
            super::TILE_GROUPS.len(),
            expect.len(),
            "seven groups (lock #8)"
        );
        for (g, (label, surfaces)) in super::TILE_GROUPS.iter().zip(expect) {
            assert_eq!(g.label, label, "group order");
            assert_eq!(
                g.surfaces, surfaces,
                "{label} membership + within-group order"
            );
        }
        // Unlike the app picker (which pulls Workbench/System/Desktop out to
        // standalone cells), lock #8 places ALL 18 Surface::ALL entries
        // inside a group — none sit outside. The compile-time guard above
        // already enforces "exactly once"; re-prove it here at runtime too
        // (the dock.rs belt-and-suspenders convention).
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
    fn all_18_tiles_render_at_one_uniform_size_and_stay_within_the_left_pane() {
        // Lock #6 — one uniform tile size for all 18, no variants — proven
        // on REAL rendered rects (the addressable-cell idiom via
        // `tile_id`), not just on the shared constants two tiles happen to
        // both reference.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);

        let left_pane_right_edge = DOCK_W + super::LEFT_PANE_W;
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
                super::tile_display_text(Surface::Bookmarks, &facts, t),
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
            super::tile_display_text(Surface::About, &[], 0.0),
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
        assert_eq!(super::rotating_fact_index(3, 0.0), 0);
        assert_eq!(super::rotating_fact_index(3, interval - 0.01), 0);
        assert_eq!(super::rotating_fact_index(3, interval), 1);
        assert_eq!(super::rotating_fact_index(3, interval * 2.0), 2);
        assert_eq!(
            super::rotating_fact_index(3, interval * 3.0),
            0,
            "wraps back to the first fact after a full cycle"
        );
    }

    #[test]
    fn rotating_fact_index_never_advances_a_0_or_1_fact_list() {
        for t in [0.0, 1.0, 100.0, 1000.0] {
            assert_eq!(super::rotating_fact_index(0, t), 0);
            assert_eq!(super::rotating_fact_index(1, t), 0);
        }
    }

    #[test]
    fn tile_display_text_rotates_through_2plus_facts_on_the_locked_interval() {
        let facts = vec!["3 unread".to_string(), "Latest: eagle".to_string()];
        let interval = super::TILE_FACT_ROTATE_INTERVAL.as_secs_f64();
        assert_eq!(
            super::tile_display_text(Surface::Chat, &facts, 0.0),
            "3 unread"
        );
        assert_eq!(
            super::tile_display_text(Surface::Chat, &facts, interval),
            "Latest: eagle"
        );
        assert_eq!(
            super::tile_display_text(Surface::Chat, &facts, interval * 2.0),
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

    #[test]
    fn a_query_filters_to_the_tiles_whose_label_matches() {
        // The pure ranking authority both the render and Enter read (so the
        // painted list and Enter's target can never diverge). Case-insensitive
        // substring over Surface::label().
        use Surface::{Phones, Terminal, Workbench};
        assert_eq!(super::search_matches("phone"), vec![Phones]);
        assert_eq!(super::search_matches("term"), vec![Terminal]);
        assert_eq!(super::search_matches("workbench"), vec![Workbench]);
        assert_eq!(
            super::search_matches("PHONES"),
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
    fn search_ranks_prefix_over_substring_over_group_name() {
        use Surface::{InfraCode, MeshView, Music, Workbench};
        // "mesh": a label PREFIX ("Mesh Map") outranks a GROUP-name-only hit
        // ("Mesh Control" → Workbench/InfraCode); ties keep Surface::ALL order.
        assert_eq!(
            super::search_matches("mesh"),
            vec![MeshView, Workbench, InfraCode],
            "the label-prefix hit leads, then the group-only hits in ALL order"
        );
        // "i": the sole label-PREFIX ("Infra as Code") ranks above every
        // label-SUBSTRING hit (Music/Media/Files/…), never buried among them.
        let by_i = super::search_matches("i");
        assert_eq!(by_i.first(), Some(&InfraCode), "the prefix match leads");
        assert!(
            by_i.len() > 1 && by_i.contains(&Music),
            "substring matches are still included, just ranked below the prefix"
        );
    }

    #[test]
    fn the_search_field_tucks_below_the_tile_grid_without_overlap() {
        // Constant-geometry assertion (the WIN7-2 `PANEL_H`/`PANEL_W`
        // precedent, no GPU): the 7-group grid must bottom out strictly above
        // the search band, and all 18 result rows must fit the results area at
        // once (worst case — a one-letter query matching every surface).
        let grid_bottom = super::PANE_PAD + super::TILE_GRID_CONTENT_H;
        let search_top = PANEL_H - super::PANE_PAD - super::SEARCH_H;
        assert!(
            grid_bottom <= search_top,
            "the tile grid ({grid_bottom}) must end above the search band ({search_top})"
        );
        let results_rows_h = search_top - Style::SP_XS - super::PANE_PAD;
        assert!(
            18.0 * super::RESULT_ROW_H <= results_rows_h,
            "all 18 result rows must fit the results band"
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
