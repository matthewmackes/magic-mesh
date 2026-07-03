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
//! Tabs, broadcast input, and the mackesd mesh PTY broker arrive in TERM-5
//! onward.

pub mod engine;
pub mod palette;
pub mod pty;
pub mod screen;
pub mod splits;
pub mod widget;

pub use engine::{Terminal, DEFAULT_SCROLLBACK};
pub use pty::{LocalPty, SpawnOptions};
pub use screen::{Cell, CellAttrs, CellColor, CursorPos, Screen};
pub use splits::{consume_commands, Command, NavDir, Pane, SessionId, SplitDir, SplitTerminal};
pub use widget::TerminalWidget;
