//! The backing a [`crate::widget::TerminalWidget`] renders.
//!
//! Three variants: a **local** PTY shell (TERM-1..6), a **remote** mesh shell
//! driven over the broker (TERM-8), or a mounted **tmux pane** on a control
//! channel (TMUX-FC-3).
//!
//! All variants expose the same tiny surface the widget needs (read the engine,
//! send input, resize, liveness), so the one TERM-3 grid renderer + input handler
//! paints any pane — no second terminal emulator (§6). A remote pane adds a
//! per-frame [`Session::poll`] (draining the Bus) and the honest node marker +
//! status chip; a local pane pumps on its own threads and has neither. A tmux
//! pane reads the shared engine `%output` feeds and routes typed input through
//! `send-keys` on the control channel — the FC round-trip in widget form (its
//! grid is resized by `%layout-change`, never by the widget's rect).

use std::io;

use mde_egui::egui::Color32;
use mde_egui::Style;

use crate::engine::Terminal;
use crate::pty::LocalPty;
use crate::remote::{RemotePty, StatusTone};
use crate::tmux::TmuxPaneIo;

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
    /// A mounted tmux pane on a control channel (TMUX-FC-3): the shared engine
    /// `%output` feeds, input routed as `send-keys`.
    Tmux(TmuxPaneIo),
}

impl Session {
    /// Drain any pending backing work for this frame. A remote session reads its
    /// Bus state log; a local session pumps on its own threads and a tmux pane
    /// is fed by the surface-level controller pump, so both are no-ops here.
    pub fn poll(&mut self) {
        if let Self::Remote(remote) = self {
            remote.poll();
        }
    }

    /// Resize the grid to `cols × rows` (and, for a remote session, publish the
    /// resize verb). A tmux pane's grid belongs to tmux — `%layout-change`
    /// resizes it, so the widget-rect resize is deliberately a no-op there (the
    /// mounted view reports the *client* size via `refresh-client` instead).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        match self {
            Self::Local(pty) => pty.resize(cols, rows),
            Self::Remote(remote) => remote.resize(cols, rows),
            Self::Tmux(_) => {}
        }
    }

    /// Install a repaint waker so backing output drives an immediate repaint
    /// instead of trailing the widget's fixed self-timer (the render-lag fix).
    /// A local PTY fires it from its reader pump on each output batch; a tmux
    /// pane is pumped by the surface-level controller (already on the egui
    /// thread), so this is a no-op there. The widget installs it once, on its
    /// first frame, passing an `egui::Context::request_repaint` closure.
    pub fn set_repaint_waker(&self, wake: impl Fn() + Send + Sync + 'static) {
        match self {
            Self::Local(pty) => pty.set_repaint_waker(wake),
            Self::Remote(_) | Self::Tmux(_) => {}
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
            Self::Tmux(io) => io.send_input(bytes),
        }
    }

    /// TMUX-FC-8 — for a tmux pane, also yank a native GUI copy into tmux's paste
    /// buffer (`set-buffer`), so `prefix ]` pastes it inside a pane; a no-op for a
    /// local/remote pane (whose copy lives only in the OS + mesh clipboard). Only
    /// a **single-line** selection is yanked — the control channel is line-based,
    /// so a multi-line one rides the clipboard alone (which has no such limit).
    pub fn yank_tmux_buffer(&self, text: &str) {
        if let Self::Tmux(io) = self {
            if !text.contains('\n') {
                let _ = io.yank_buffer(text);
            }
        }
    }

    /// Run `f` against the current engine state (the render-agnostic snapshot
    /// source every variant shares).
    pub fn with_terminal<R>(&self, f: impl FnOnce(&Terminal) -> R) -> R {
        match self {
            Self::Local(pty) => pty.with_terminal(f),
            Self::Remote(remote) => remote.with_terminal(f),
            Self::Tmux(io) => io.with_terminal(f),
        }
    }

    /// Whether the pane should reap (close). A local session reaps on child exit;
    /// a remote session reaps only on a **clean** shell exit (a failure lingers so
    /// its reason stays on screen). A tmux pane's life belongs to tmux — the
    /// mounted view unmounts it when the layout drops it — so it only reaps once
    /// the whole control channel is gone.
    #[must_use]
    pub fn is_output_closed(&self) -> bool {
        match self {
            Self::Local(pty) => pty.is_output_closed(),
            Self::Remote(remote) => remote.is_output_closed(),
            Self::Tmux(io) => io.is_closed(),
        }
    }

    /// The local PTY, when this is a local session (the reap/child-pid tests read
    /// through it).
    #[must_use]
    pub const fn local(&self) -> Option<&LocalPty> {
        match self {
            Self::Local(pty) => Some(pty),
            Self::Remote(_) | Self::Tmux(_) => None,
        }
    }

    /// The remote shell, when this is a remote session (TERM-10 layout capture
    /// reads its peer + node marker through it).
    #[must_use]
    pub fn remote(&self) -> Option<&RemotePty> {
        match self {
            Self::Remote(remote) => Some(remote.as_ref()),
            Self::Local(_) | Self::Tmux(_) => None,
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
            Self::Tmux(io) => {
                let ended = io.is_closed();
                RenderState {
                    live: !ended,
                    node: None,
                    note: ended.then(|| ("tmux detached".to_string(), Style::TEXT_DIM)),
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

#[cfg(test)]
mod tests {
    use super::Session;
    use crate::engine::Terminal;
    use crate::pty::{LocalPty, SpawnOptions};
    use crate::tmux::{CommandSink, TmuxPaneIo};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::{Duration, Instant};

    #[test]
    fn tmux_pane_yanks_a_single_line_selection_into_the_buffer_and_skips_multiline() {
        // TMUX-FC-8 — a tmux pane's copy also yanks into tmux's buffer, but only a
        // single-line selection (the control channel is line-based); a multi-line
        // one rides the clipboard alone.
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let engine = Arc::new(Mutex::new(Terminal::with_default_scrollback(20, 4)));
        let session = Session::Tmux(TmuxPaneIo::new(3, engine, CommandSink::for_tests(tx)));

        session.yank_tmux_buffer("one line");
        assert_eq!(rx.recv().expect("yank"), b"set-buffer -- 'one line'\n");

        // A multi-line selection yanks nothing to tmux (no set-buffer emitted).
        session.yank_tmux_buffer("line one\nline two");
        assert!(
            rx.try_recv().is_err(),
            "a multi-line selection must not reach the line-based control channel"
        );
    }

    #[test]
    fn set_repaint_waker_is_a_noop_on_a_tmux_pane() {
        // A tmux pane is pumped by the surface-level controller (already on the
        // egui thread), so it stores no waker — the render-lag fix's dispatch is
        // a no-op there: the call neither panics nor ever fires the closure.
        let (tx, _rx) = mpsc::channel::<Vec<u8>>();
        let engine = Arc::new(Mutex::new(Terminal::with_default_scrollback(20, 4)));
        let session = Session::Tmux(TmuxPaneIo::new(3, engine, CommandSink::for_tests(tx)));

        let ticks = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&ticks);
        session.set_repaint_waker(move || {
            seen.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(
            ticks.load(Ordering::Relaxed),
            0,
            "a tmux pane must not fire a repaint waker"
        );
    }

    #[test]
    fn set_repaint_waker_reaches_a_local_pty() {
        // A `Local` session dispatches the waker into its PTY, whose reader pump
        // fires it on output — the render-lag fix reaching the backing.
        let session = Session::Local(
            LocalPty::spawn(SpawnOptions {
                shell: Some("/bin/sh".to_owned()),
                ..SpawnOptions::default()
            })
            .expect("spawn /bin/sh"),
        );
        let ticks = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&ticks);
        session.set_repaint_waker(move || {
            seen.fetch_add(1, Ordering::Relaxed);
        });
        session
            .send_input(b"echo reaches-'local'\n")
            .expect("queue input");

        // Wait on observed engine state (the pumps are async), then the waker
        // must have advanced — the dispatch reached the pty's reader pump.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let seen_output = session.with_terminal(|term| {
                let full = term.full();
                (0..full.rows()).any(|row| full.line_text(row).contains("reaches-local"))
            });
            if seen_output {
                break;
            }
            assert!(Instant::now() < deadline, "timed out waiting for output");
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            ticks.load(Ordering::Relaxed) > 0,
            "set_repaint_waker on a Local session must reach the pty's reader pump"
        );
    }
}
