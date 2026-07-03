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
//!
//! The egui surface, the mackesd mesh PTY broker, splits, tabs, and broadcast
//! arrive in TERM-3 onward.

pub mod engine;
pub mod pty;
pub mod screen;

pub use engine::{Terminal, DEFAULT_SCROLLBACK};
pub use pty::{LocalPty, SpawnOptions};
pub use screen::{Cell, CellAttrs, CellColor, CursorPos, Screen};
