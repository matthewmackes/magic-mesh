//! The backing a [`crate::widget::TerminalWidget`] renders — a **local** PTY
//! shell (TERM-1..6) or a **remote** mesh shell driven over the broker (TERM-8).
//!
//! Both variants expose the same tiny surface the widget needs (read the engine,
//! send input, resize, liveness), so the one TERM-3 grid renderer + input handler
//! paints either pane — no second terminal emulator (§6). A remote pane adds a
//! per-frame [`Session::poll`] (draining the Bus) and the honest node marker +
//! status chip; a local pane pumps on its own threads and has neither.

use std::io;

use mde_egui::egui::Color32;
use mde_egui::Style;

use crate::engine::Terminal;
use crate::pty::LocalPty;
use crate::remote::{RemotePty, StatusTone};

/// What the widget needs to paint a frame's chrome around the grid.
///
/// Whether the session is still live (cursor + repaint heartbeat), an optional
/// node marker (remote panes), and an optional honest status note.
pub struct RenderState {
    /// The session is still moving — paint the cursor + keep repainting/polling.
    pub live: bool,
    /// The node marker label, for a remote pane (`None` for local).
    pub node: Option<String>,
    /// An honest status chip (text + resolved §4 colour), or `None` when plainly
    /// live.
    pub note: Option<(String, Color32)>,
}

/// The backing of one terminal pane.
///
/// A [`RemotePty`] owns a full VT engine (much larger than a [`LocalPty`], whose
/// engine sits behind an `Arc`), so it is boxed to keep the enum — and every
/// pane in the split registry — a uniform pointer size.
pub enum Session {
    /// A local login shell on a real PTY (TERM-2).
    Local(LocalPty),
    /// A remote mesh shell driven over the TERM-7 broker (TERM-8).
    Remote(Box<RemotePty>),
}

impl Session {
    /// Drain any pending backing work for this frame. A remote session reads its
    /// Bus state log; a local session pumps on its own threads, so this is a
    /// no-op for it.
    pub fn poll(&mut self) {
        if let Self::Remote(remote) = self {
            remote.poll();
        }
    }

    /// Resize the grid to `cols × rows` (and, for a remote session, publish the
    /// resize verb).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        match self {
            Self::Local(pty) => pty.resize(cols, rows),
            Self::Remote(remote) => remote.resize(cols, rows),
        }
    }

    /// Send input bytes to the shell.
    ///
    /// # Errors
    /// [`io::ErrorKind::BrokenPipe`] once the session's write side is gone.
    pub fn send_input(&self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Local(pty) => pty.send_input(bytes),
            Self::Remote(remote) => remote.send_input(bytes),
        }
    }

    /// Run `f` against the current engine state (the render-agnostic snapshot
    /// source both variants share).
    pub fn with_terminal<R>(&self, f: impl FnOnce(&Terminal) -> R) -> R {
        match self {
            Self::Local(pty) => pty.with_terminal(f),
            Self::Remote(remote) => remote.with_terminal(f),
        }
    }

    /// Whether the pane should reap (close). A local session reaps on child exit;
    /// a remote session reaps only on a **clean** shell exit (a failure lingers so
    /// its reason stays on screen).
    #[must_use]
    pub fn is_output_closed(&self) -> bool {
        match self {
            Self::Local(pty) => pty.is_output_closed(),
            Self::Remote(remote) => remote.is_output_closed(),
        }
    }

    /// The local PTY, when this is a local session (the reap/child-pid tests read
    /// through it).
    #[must_use]
    pub const fn local(&self) -> Option<&LocalPty> {
        match self {
            Self::Local(pty) => Some(pty),
            Self::Remote(_) => None,
        }
    }

    /// The remote shell, when this is a remote session (TERM-10 layout capture
    /// reads its peer + node marker through it).
    #[must_use]
    pub fn remote(&self) -> Option<&RemotePty> {
        match self {
            Self::Remote(remote) => Some(remote.as_ref()),
            Self::Local(_) => None,
        }
    }

    /// This frame's render chrome — liveness, node marker, honest status note.
    #[must_use]
    pub fn render_state(&self) -> RenderState {
        match self {
            Self::Local(pty) => {
                let ended = pty.is_output_closed();
                RenderState {
                    live: !ended,
                    node: None,
                    note: ended.then(|| ("session ended".to_string(), Style::TEXT_DIM)),
                }
            }
            Self::Remote(remote) => {
                let status = remote.status();
                RenderState {
                    live: status.is_live(),
                    node: Some(remote.node_label().to_string()),
                    note: status.note().map(|(text, tone)| (text, tone_color(tone))),
                }
            }
        }
    }
}

/// Map a remote status tone to its `Style` token (§4 — the colour mapping lives
/// at the render boundary, keeping `remote` egui-free + headless-testable).
const fn tone_color(tone: StatusTone) -> Color32 {
    match tone {
        StatusTone::Neutral => Style::ACCENT,
        StatusTone::Warn => Style::WARN,
        StatusTone::Danger => Style::DANGER,
        StatusTone::Dim => Style::TEXT_DIM,
    }
}
