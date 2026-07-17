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
//!   carve-out): Quazar-token-derived where a token carries the meaning,
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
//!   Quazar default derived from `Style` tokens, and [`presets`] holds the
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
//!   `Color32`). [`fonts`] bundles **Intel One Mono** (a clean monospace; its
//!   opt-in `ss01` ligatures are never activated) in the crate and registers it
//!   as the grid's monospace face.
//!
//! - [`menu`] (TERM-15) — the **selection context menu**: user-defined
//!   [`menu::CustomCommand`]s (a label + a `{}`/`%s` template the selection
//!   substitutes into an argv, Terminator parity, run by [`menu::CommandRunner`])
//!   plus four built-in mesh actions on the selection, each reusing an existing
//!   surface-launch verb (§6) — send-to-Chat over the NOTIFY-CHAT
//!   [`menu::ACTION_CHAT_SEND`] verb ([`menu::ChatBus`]), open-path-in-Files /
//!   open-URL-in-browser over the TERM-9 [`smart::LaunchBus`], and
//!   new-terminal-here over the TERM-4/5 [`splits`] spawn inheriting the pane's
//!   cwd. The widget renders it on the grid's right-click; the split multiplexer
//!   drains the new-terminal-here flag.
//!
//! - [`tmux`] (TMUX-FC-1) — the **control-mode core**: make tmux first-class via
//!   `iTerm2`-style control mode (`tmux -CC`). [`tmux::ControlChannel`] spawns the
//!   control client on the same `alacritty_terminal::tty` PTY seam [`pty`] uses
//!   (§6), pumping its raw control protocol into [`tmux::Parser`] — an
//!   incremental, partial-read-robust parser of the `%`-notifications
//!   ([`tmux::Notification`]). [`tmux::parse_layout`] turns tmux's layout string
//!   into a pane tree that folds onto the native [`splits::Pane`] verbatim, and
//!   [`tmux::TmuxModel`] keeps a live sessions→windows→panes model — each pane an
//!   [`engine::Terminal`] fed by `%output` (the grid a [`widget::TerminalWidget`]
//!   renders). [`tmux::commands`] is the command sink (GUI intent → a `tmux`
//!   command line); [`tmux::TmuxController`] ties it together with an honest
//!   [`tmux::Status`] (no fake attach). The chrome/session/mesh/preset mount is
//!   TMUX-FC-2..8.
//! - [`tmux_ui`] (TMUX-FC-2) — the **session management chrome**: the
//!   sessions→windows→panes sidebar tree ([`tmux_ui::TmuxChrome`]) over the live
//!   [`tmux::TmuxModel`], the full session ops (create · attach · detach · kill ·
//!   rename) each a [`tmux_ui::TmuxIntent`] mapped by [`tmux_ui::command_for`] to
//!   the real [`tmux::commands`] line (round-trip: the `%`-event reconciles the
//!   tree, never a direct mutation), and the all-sessions picker (attached AND
//!   detached, from the control-channel `list-sessions` reply). The [`menubar`]
//!   tmux menu ([`tmux_ui::TmuxMenuChoice`]) is its entry point. **TMUX-FC-4**
//!   adds the native chrome: the Quazar status bar (session · window list ·
//!   clock, ignoring tmux `status-*` config), the op toolbar, the curated
//!   ~30-command fuzzy palette, and the enriched tab/pane context menus —
//!   including the `join-pane -h` (beside) trigger — every affordance a tmux
//!   command through the same round-trip.
//! - [`menubar`] (TERM-MENUBAR-1) — the **top menu bar**: an `egui::menu::bar`
//!   of File / Edit / View / Terminal / Splits / Tabs / Session / Help
//!   drop-downs, each item the mouse twin of an existing seam (tabs / splits /
//!   search / appearance / presets / remote roster / bell / keymap), with its
//!   live shortcut beside it and context-gated items honestly greyed (§7). The
//!   embed mounts it above the tab bar; a future tmux menu (TMUX-FC) slots in.
//!
//! - **CONSOLE-2** — the Console front door's terminal seam: the shell's Start
//!   Menu launches operational entries as **named command tabs** here.
//!   [`TabbedTerminal::spawn_tab`] (surfaced on the embed as
//!   [`panel::TerminalSurface::spawn_tab`]) opens a named tab running a typed
//!   argv on a fresh PTY ([`pty::LocalPty::spawn_argv`], §9 — never a shell
//!   string); the pane **stays open on exit** with the harvested exit status
//!   ([`pty::ChildExit`]) and a press-a-key/click-to-close prompt. Root ops
//!   wrap their argv in [`tabs::sudo_argv`] — sudo prompts interactively in
//!   the tab's PTY (design lock #29, no caching tricks).

pub mod appearance;
pub mod bell;
pub mod blueprint;
pub mod engine;
pub mod fonts;
pub mod keymap;
pub mod layout;
pub mod layout_ui;
pub mod menu;
pub mod menubar;
pub mod mesh_tmux;
pub mod mouse;
pub mod notify;
mod overlay;
pub mod palette;
pub mod panel;
pub mod picker;
pub mod presets;
pub mod presets_tmux;
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
pub mod tmux;
pub mod tmux_config;
pub mod tmux_store;
pub mod tmux_ui;
pub mod watch;
pub mod widget;

pub use appearance::{Appearance, AppearancePicker, CursorShape};
pub use bell::{Bell, BellConfig, BellEffect};
pub use blueprint::{Blueprint, BlueprintPane, BlueprintWindow};
pub use engine::{TermEvent, Terminal, DEFAULT_SCROLLBACK};
pub use keymap::{Action, Chord, Keymap, KeymapConfig};
pub use layout::{LayoutPane, LayoutStore, LayoutTab, PaneSpec, SavedLayout};
pub use layout_ui::{LayoutIntent, LayoutManager};
pub use menu::{
    BusChatClient, ChatBus, CommandRunner, ContextMenu, CustomCommand, OsCommandRunner,
    ACTION_CHAT_SEND,
};
pub use menubar::{BellMode, Gate, MenuAction, MenuBar, MenuContext};
pub use mesh_tmux::{attach_command, MeshControlChannel};
pub use mouse::{encode_sgr, MouseButton, MouseEvent};
pub use notify::{BusNotifyClient, NoticeLevel, NotifyBus, TermNotice, TOAST_TOPIC};
pub use palette::Palette;
pub use panel::{real_terminal, terminal_panel, terminal_pump, TerminalSurface};
pub use picker::{RemotePicker, RemoteTarget};
pub use presets::Preset;
pub use presets_tmux::MeshPreset;
pub use pty::{ChildExit, LocalPty, SpawnOptions};
pub use remote::{BusPtyClient, PtyBus, RemotePty, RemoteStatus};
pub use roster::{BusRoster, PeerEntry, Presence, RosterClient, RosterSnapshot};
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
pub use tabs::{consume_tab_commands, sudo_argv, RemoteHub, TabCommand, TabbedTerminal};
pub use title::PaneTitle;
pub use tmux::{
    commands as tmux_commands, parse_layout, parse_pane_titles, parse_session_list,
    parse_window_order, resize_for_divider, CommandSink, ControlChannel, ControlLink, Layout,
    LayoutDir, LayoutError, LayoutKind, Notification, PaneResize, Parser as TmuxParser, ResizeDir,
    SessionInfo, Status as TmuxStatus, StockLayout, TmuxController, TmuxLaunch, TmuxModel,
    TmuxPane, TmuxPaneIo, TmuxSession, TmuxWindow,
};
pub use tmux_config::{TmuxConfig, TmuxConfigStore};
pub use tmux_store::{session_safe, SessionTemplate, TmuxState, TmuxStateStore};
pub use tmux_ui::{command_for, TmuxChrome, TmuxIntent, TmuxMenuChoice};
pub use watch::{ActivityWatch, WatchEvent, WatchMode};
pub use widget::{ClipboardOptions, TerminalWidget};
