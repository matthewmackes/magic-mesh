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
//! The mackesd mesh PTY broker arrives in TERM-7 onward.

pub mod engine;
pub mod palette;
pub mod pty;
pub mod screen;
pub mod splits;
pub mod tabs;
pub mod widget;

pub use engine::{Terminal, DEFAULT_SCROLLBACK};
pub use pty::{LocalPty, SpawnOptions};
pub use screen::{Cell, CellAttrs, CellColor, CursorPos, Screen};
pub use splits::{
    consume_commands, Broadcast, Command, NavDir, Pane, SessionId, SplitDir, SplitTerminal,
};
pub use tabs::{consume_tab_commands, TabCommand, TabbedTerminal};
pub use widget::TerminalWidget;
