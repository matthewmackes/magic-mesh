//! `mde-term-egui` — the mesh terminal surface.
//!
//! This crate builds toward a Terminator-class egui-native terminal (design:
//! `docs/design/mesh-terminal.md`). TERM-1 lands the **VT engine core**: a
//! mature xterm/VT100 engine ([`alacritty_terminal`], §6 — never a re-implemented
//! parser) wrapped behind a render-agnostic screen model.
//!
//! - [`engine::Terminal`] — feed it PTY/ANSI bytes, read a [`screen::Screen`] out.
//!   It owns the cell grid and the soft-capped scrollback ring; all VT semantics
//!   (SGR, cursor motion, clears, wrapping, tab stops, scroll-off) are the
//!   engine's.
//! - [`screen`] — the flat, immutable [`screen::Screen`] snapshot (a [`screen::Cell`]
//!   grid + cursor) that later units render (the egui pane, TERM-3) and search
//!   (scrollback, TERM-9), with no engine or toolkit types on its surface.
//! - [`pty::LocalPty`] (TERM-2) — a real local login shell (`$SHELL`, fallback
//!   `/bin/sh`) on a fresh PTY, pumped into the engine by reader/writer
//!   threads; typed argv spawn (§9), `TIOCSWINSZ` on resize, clean child reap
//!   on close.
//! - [`widget::TerminalWidget`] (TERM-3) — the interactive egui pane: the cell
//!   grid painted as batched same-style runs (fg/bg/attrs through the content
//!   palette), block cursor, mouse selection + clipboard, a scrollback
//!   viewport, and rect→cols/rows resizing wired to the PTY. The `mde-term-egui`
//!   binary mounts one over a login shell on the shared harness.
//! - [`splits`] (TERM-4) — Terminator's split model: a pure
//!   `Leaf | Split { dir, ratio, a, b }` binary tree (split to any depth,
//!   close-collapses, drag-reparent — all unit-tested headless) rendered by
//!   [`splits::SplitTerminal`], which multiplexes one TERM-3 widget per leaf
//!   over a session registry. Draggable Style-token dividers, zoom
//!   (maximize/restore), Alt-drag rearrange, and focus that follows clicks,
//!   splits, closes and `Alt+arrow` navigation. The binary now mounts it.
//! - [`palette`] — the 16/256-colour **content** palette (the documented §4
//!   carve-out): Quasar-token-derived where a token carries the meaning,
//!   standard ANSI hues elsewhere; the only raw colour values in the crate.
//!
//! - [`tabs`] (TERM-5) — a tab layer over the splits: [`tabs::TabbedTerminal`]
//!   owns one [`splits::SplitTerminal`] per tab (each an independent split tree)
//!   plus the active index, with a Carbon-token tab bar (new/close/reorder) and
//!   the Terminator tab chords. Switching tabs preserves each tab's whole
//!   layout + live PTYs; the last tab closing empties the surface. The binary
//!   mounts it above the splits.
//!
//! - [`splits`] also carries **broadcast/grouped input** (TERM-6): the focused
//!   pane's typing fans out to every pane ([`Broadcast::All`]) or to the panes
//!   sharing its named group ([`Broadcast::Group`]), each replayed through the
//!   target pane's own [`LocalPty`] write path (§6 — no PTY write is
//!   re-implemented). The panes in the live set wear a `Style::WARN` border;
//!   the mode toggles by `Ctrl+Shift+A`/`Ctrl+Shift+G` or the on-surface chip,
//!   and panes are assigned to named groups from a per-pane badge.
//!
//! - [`remote`] (TERM-8) — the desktop half of the TERM-7 mesh PTY-broker
//!   contract: [`remote::RemotePty`] opens a shell on a mesh peer over the Bus
//!   (`action/pty/<peer>` verbs) and streams its append-log state
//!   (`state/pty/<id>`, base64 chunks) into the **same** reused VT engine + grid,
//!   with honest connecting / reconnecting / unreachable / closed states (§7).
//!   [`roster`] mirrors the presence roster and [`picker::RemotePicker`] is the
//!   "new terminal on → <peer>" picker + manual host entry; [`session::Session`]
//!   is the local-or-remote backing the one [`widget::TerminalWidget`] renders.
//!
//! - [`search`] + [`smart`] (TERM-9) — scrollback search + a smart clipboard,
//!   both pure folds over the reused grid/scrollback [`screen::Screen`] (§6).
//!   [`search::Search`] finds a literal-or-regex query row-by-row with smart
//!   case, next/prev, and wrap; the widget highlights the hits through `Style`
//!   tokens and scrolls the current one into view. [`smart`] classifies the
//!   token under a double-click (word / URL / path) or a triple-click (line),
//!   drives copy-on-select + middle-click paste, and — per design lock Q12 —
//!   routes a Ctrl-clicked URL to the Bookmarks browser / a path to the Files
//!   surface over the Bus ([`smart::LaunchBus`], published on
//!   [`smart::OPEN_TOPIC`]).
//!
//! - [`layout`] + [`layout_ui`] (TERM-10) — **mesh-synced saved layouts**. A
//!   [`layout::SavedLayout`] is the serializable projection of a whole surface:
//!   every tab's split tree (mirroring [`splits::Pane`] + reusing
//!   [`splits::SplitDir`]) with, per pane, a relaunch [`layout::PaneSpec`] — a
//!   local pane's cwd + command, or a remote pane's [`RemoteTarget`].
//!   [`TabbedTerminal::capture_layout`] reads the live surface into one and
//!   [`TabbedTerminal::launch_layout`] rebuilds it — respawning local shells and
//!   reconnecting remote panes through the same TERM-7/8 broker path. Persistence
//!   is the bookmarks idiom reused verbatim ([`layout::LayoutStore`]): a
//!   single-writer-per-node directory under the Syncthing-replicated workgroup
//!   root, so a layout saved on one node is launchable on another once synced.
//!   [`layout_ui::LayoutManager`] is the save/launch overlay (`Ctrl+Shift+L`).
//!
//! - [`palette`] + [`presets`] + [`appearance`] (TERM-11) — **palette + look**.
//!   [`palette::Palette`] is the runtime content colour scheme (the 16 ANSI slots
//!   with the default fg/bg/cursor); [`palette::Palette::from_tokens`] is the
//!   Quasar default derived from `Style` tokens, and [`presets`] holds the
//!   bundled classics (Solarized dark/light, Gruvbox, Nord) — the one sanctioned
//!   home for their defining hex. [`appearance::Appearance`] bundles the three
//!   knobs (scheme, font size, cursor style) and [`appearance::AppearancePicker`]
//!   is the simple picker (`Ctrl+Shift+P`); the surface pushes the appearance
//!   into every pane each frame, so a change reaches every live shell at once.
//!
//! - [`mouse`] + [`fonts`] (TERM-13) — **TUI fidelity**. [`mouse`] encodes egui
//!   pointer activity into the xterm **SGR (1006)** mouse reports a program in
//!   mouse-mode reads (click / drag / scroll / hover), which the widget forwards
//!   to the PTY only when the engine says the app enabled tracking — with a
//!   **Shift-bypass** so Shift+drag always does native text selection. 24-bit
//!   true-colour + 256-colour already pass through the engine → [`screen::Cell`]
//!   → [`palette::cell_colors`] un-quantized (`Rgb`/`Palette` straight to
//!   `Color32`). [`fonts`] bundles **Fira Code** (programming ligatures) in the
//!   crate and registers it as the grid's monospace face.

pub mod appearance;
pub mod bell;
pub mod engine;
pub mod fonts;
pub mod keymap;
pub mod layout;
pub mod layout_ui;
pub mod mouse;
pub mod notify;
pub mod palette;
pub mod picker;
pub mod presets;
pub mod pty;
pub mod remote;
pub mod roster;
pub mod screen;
pub mod search;
pub mod session;
pub mod smart;
pub mod splits;
pub mod tabs;
pub mod title;
pub mod watch;
pub mod widget;

pub use appearance::{Appearance, AppearancePicker, CursorShape};
pub use bell::{Bell, BellConfig, BellEffect};
pub use engine::{TermEvent, Terminal, DEFAULT_SCROLLBACK};
pub use keymap::{Action, Chord, Keymap, KeymapConfig};
pub use layout::{LayoutPane, LayoutStore, LayoutTab, PaneSpec, SavedLayout};
pub use layout_ui::{LayoutIntent, LayoutManager};
pub use mouse::{encode_sgr, MouseButton, MouseEvent};
pub use notify::{BusNotifyClient, NoticeLevel, NotifyBus, TermNotice, TOAST_TOPIC};
pub use palette::Palette;
pub use picker::{RemotePicker, RemoteTarget};
pub use presets::Preset;
pub use pty::{LocalPty, SpawnOptions};
pub use remote::{BusPtyClient, PtyBus, RemotePty, RemoteStatus};
pub use roster::{BusRoster, Presence, RosterClient, RosterSnapshot};
pub use screen::{Cell, CellAttrs, CellColor, CursorPos, Screen};
pub use search::{CaseMode, Match, Search};
pub use session::Session;
pub use smart::{
    detect_launch, line_span, route, smart_span, BusLaunchClient, LaunchBus, LaunchRequest,
    LaunchRoute, SmartKind, OPEN_TOPIC,
};
pub use splits::{
    consume_commands, Broadcast, Command, NavDir, Pane, SessionId, SplitDir, SplitTerminal,
};
pub use tabs::{consume_tab_commands, RemoteHub, TabCommand, TabbedTerminal};
pub use title::PaneTitle;
pub use watch::{ActivityWatch, WatchEvent, WatchMode};
pub use widget::{ClipboardOptions, TerminalWidget};
