//! **Control-mode core** (TMUX-FC-1): make tmux a first-class citizen of the
//! terminal via `iTerm2`-style **control mode** (the `tmux -CC` protocol).
//!
//! Design: `docs/design/tmux-first-class.md` (lock #1 — the deepest integration:
//! tmux windows become the terminal's native tabs, panes its native splits, and
//! GUI ops round-trip through tmux). This module is the FOUNDATION the later
//! units (chrome, sessions, mesh, presets — TMUX-FC-2..8) mount on. It carries:
//!
//! * [`ControlChannel`] — spawns `tmux -CC` on a real PTY through the exact
//!   `alacritty_terminal::tty` seam [`crate::pty`] uses (§6 — one PTY layer, no
//!   second one), but pumps its output as **raw control-protocol bytes** into a
//!   parser instead of a VT engine (the control stream is a line protocol, not
//!   screen output — feeding it to the VT engine would mangle it).
//! * [`Parser`] — an **incremental, partial-read-robust** control-mode protocol
//!   parser. It buffers bytes until a full `\n`-terminated line, frames
//!   `%begin`/`%end`/`%error` command replies, and decodes the `%`-notifications
//!   ([`Notification`]): `%output` (octal-unescaped pane bytes), window
//!   add/close/renamed, `%layout-change`, session-changed, the unlinked-window
//!   trio, `%window-pane-changed`, `%pane-mode-changed`, `%exit`, and more —
//!   any unrecognised `%`-line is preserved as [`Notification::Other`], never a
//!   crash.
//! * [`parse_layout`] — the tmux **layout string** → a [`Layout`] pane tree, and
//!   [`Layout::to_pane_tree`] folds it into the native [`crate::splits::Pane`]
//!   binary tree (sizes → ratios) so tmux's arrangement drives the native
//!   splits **verbatim** (§6 — reuse TERM-4, not a re-implementation).
//! * [`TmuxModel`] — the live model of **sessions → windows → panes**. Each pane
//!   owns an [`crate::engine::Terminal`] (the same grid a [`crate::widget::TerminalWidget`]
//!   renders); `%output` feeds it, so mapping a pane to a native leaf is "hand
//!   the renderer this engine". [`TmuxModel::apply`] reconciles the model from
//!   notifications — window-add/close, layout-change (the pane-set + arrangement
//!   truth), session-changed — never by mutating a native tree directly.
//! * [`commands`] — the **command sink**: pure builders turning a GUI intent
//!   (select / split / resize / kill / rename / send-keys) into the `tmux`
//!   command string written to the control channel. The resulting `%`-event is
//!   what reconciles the model — the round-trip the design's risk section
//!   demands.
//! * [`TmuxController`] — ties a live [`ControlChannel`] to the [`Parser`] +
//!   [`TmuxModel`], with an honest [`Status`] (Connecting / Attached / Error /
//!   Exited — no fake attach if `tmux` is absent or `-CC` fails).
//!
//! ## Landed on top (TMUX-FC-2/3) and seams left for TMUX-FC-4..8
//! - TMUX-FC-2: the sidebar tree + session ops + the all-sessions picker
//!   (`crate::tmux_ui`), fed by [`commands`]' session builders +
//!   [`parse_session_list`].
//! - TMUX-FC-3 (here): the **window & pane op** builders (split / close / zoom ·
//!   break / join / swap / move · resize · rename), the zoom + window-order +
//!   pane-title model truth ([`parse_pane_titles`] / [`parse_window_order`] over
//!   tagged `list-*` replies — reconciliation stays server-driven), the
//!   divider-drag → `resize-pane` mapping ([`resize_for_divider`]), and the
//!   **pane-content mount seam**: [`CommandSink`] + [`TmuxPaneIo`] back a
//!   [`crate::session::Session::Tmux`] widget that reads the shared engine and
//!   routes typed input to [`commands::send_keys`].
//! - Still open: native status bar + toolbar + palette + context menus
//!   (TMUX-FC-4), persistence/templates (FC-5), mesh attach (FC-6), presets
//!   (FC-7), config/keys/copy (FC-8).

#![allow(clippy::module_name_repetitions)]

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty;

use crate::engine::Terminal;
use crate::splits::{clamp_ratio, NodePath, Pane as SplitPane, SessionId, SplitDir};

/// Read chunk for the control-channel pump (one kernel PTY buffer, ~8 KiB).
const CTRL_READ_CHUNK: usize = 8192;

/// The default grid a freshly created tmux pane's engine opens at, before its
/// first `%layout-change` sizes it to the real cell rectangle.
const DEFAULT_PANE_COLS: u16 = 80;
/// See [`DEFAULT_PANE_COLS`].
const DEFAULT_PANE_ROWS: u16 = 24;

/// Lock a mutex, riding through poisoning (a panicked pump thread must not wedge
/// the surface) — the same discipline [`crate::pty`] uses.
fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ─────────────────────────────────────────────────────────────────────────────
// The live control channel: `tmux -CC` on a real PTY, pumped as raw bytes.
// ─────────────────────────────────────────────────────────────────────────────

/// How the `tmux -CC` control client is launched.
///
/// The default is "attach the `main` session, creating it if absent" — the
/// design's "new-session or attach" in one (`new-session -A`). A caller wanting
/// a private server (tests, an isolated preset) supplies its own argv.
#[derive(Clone, Debug)]
pub struct TmuxLaunch {
    /// The tmux binary (resolved on `$PATH`); typically `"tmux"`.
    pub bin: String,
    /// The argv after the binary. Must include `-CC` (control mode). The
    /// default is `["-CC", "new-session", "-A", "-s", "main"]`.
    pub args: Vec<String>,
    /// The control client's grid columns — tmux sizes new windows to the
    /// attaching client, so this is the initial window width.
    pub cols: u16,
    /// The control client's grid rows (see [`Self::cols`]).
    pub rows: u16,
    /// Working directory for the tmux process (`None` inherits the caller's).
    pub cwd: Option<PathBuf>,
    /// Extra environment layered onto the inherited process env.
    pub env: Vec<(String, String)>,
}

impl Default for TmuxLaunch {
    fn default() -> Self {
        Self {
            bin: "tmux".to_owned(),
            args: ["-CC", "new-session", "-A", "-s", "main"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            cols: DEFAULT_PANE_COLS,
            rows: DEFAULT_PANE_ROWS,
            cwd: None,
            env: Vec::new(),
        }
    }
}

impl TmuxLaunch {
    /// Attach the named session, creating it if it does not exist
    /// (`new-session -A -s <name>`), in control mode.
    #[must_use]
    pub fn session(name: &str) -> Self {
        Self {
            args: [
                "-CC".to_owned(),
                "new-session".to_owned(),
                "-A".to_owned(),
                "-s".to_owned(),
                name.to_owned(),
            ]
            .to_vec(),
            ..Self::default()
        }
    }
}

/// A live `tmux -CC` control channel on a real PTY.
///
/// Reuses the `alacritty_terminal::tty` PTY layer (§6, exactly as
/// [`crate::pty::LocalPty`]) but its reader pump delivers **raw** protocol bytes
/// to an internal queue — the control stream is a line protocol, not screen
/// output, so it never touches a VT engine. Dropping the channel closes the
/// input queue, releases the PTY (SIGHUP + reap of the tmux client), and joins
/// both pump threads.
pub struct ControlChannel {
    pty: Arc<Mutex<Option<tty::Pty>>>,
    input_tx: Arc<Mutex<Option<Sender<Vec<u8>>>>>,
    output_rx: Receiver<Vec<u8>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    output_closed: Arc<AtomicBool>,
    child_pid: u32,
}

impl ControlChannel {
    /// Spawn the control client and start its raw pumps.
    ///
    /// # Errors
    /// Whatever the OS refuses: a missing `tmux` binary (`NotFound`), `openpty`
    /// failure, or fd duplication for the pump threads.
    pub fn spawn(launch: &TmuxLaunch) -> std::io::Result<Self> {
        let cols = launch.cols.max(1);
        let rows = launch.rows.max(1);

        let mut child_env = vec![
            ("TERM".to_owned(), "xterm-256color".to_owned()),
            ("COLORTERM".to_owned(), "truecolor".to_owned()),
        ];
        child_env.extend(launch.env.iter().cloned());

        let config = tty::Options {
            shell: Some(tty::Shell::new(launch.bin.clone(), launch.args.clone())),
            working_directory: launch.cwd.clone(),
            hold: false,
            env: child_env.into_iter().collect(),
        };
        let window_size = WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width: 0,
            cell_height: 0,
        };

        let pty = tty::new(&config, window_size, 0)?;
        let child_pid = pty.child().id();

        // `tty::new` force-sets the master non-blocking (alacritty polls it); our
        // pumps are dedicated blocking threads, so flip it back.
        let flags = rustix::fs::fcntl_getfl(pty.file())?;
        rustix::fs::fcntl_setfl(pty.file(), flags - rustix::fs::OFlags::NONBLOCK)?;

        let reader_file = pty.file().try_clone()?;
        let writer_file = pty.file().try_clone()?;

        let pty = Arc::new(Mutex::new(Some(pty)));
        let output_closed = Arc::new(AtomicBool::new(false));
        let (out_tx, output_rx) = mpsc::channel::<Vec<u8>>();
        let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>();
        let input_tx = Arc::new(Mutex::new(Some(in_tx)));

        let reader = thread::Builder::new()
            .name("mde-tmux-ctrl-read".into())
            .spawn({
                let pty = Arc::clone(&pty);
                let input_tx = Arc::clone(&input_tx);
                let output_closed = Arc::clone(&output_closed);
                move || {
                    pump_raw(reader_file, &out_tx, &output_closed);
                    // The client is gone — release the PTY (reap) and close the
                    // input queue so the writer pump exits and sends report the
                    // death honestly.
                    drop(lock_unpoisoned(&pty).take());
                    drop(lock_unpoisoned(&input_tx).take());
                }
            })?;

        let writer = thread::Builder::new()
            .name("mde-tmux-ctrl-write".into())
            .spawn(move || pump_input(writer_file, &in_rx))?;

        Ok(Self {
            pty,
            input_tx,
            output_rx,
            reader: Some(reader),
            writer: Some(writer),
            output_closed,
            child_pid,
        })
    }

    /// Take the next raw output chunk, or `None` when none is queued yet.
    #[must_use]
    pub fn try_recv(&self) -> Option<Vec<u8>> {
        self.output_rx.try_recv().ok()
    }

    /// `true` once the control stream has ended (the tmux client exited or the
    /// master closed) — no further output will arrive.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.output_closed.load(Ordering::Acquire)
    }

    /// Write one control-mode command line (a trailing `\n` is added).
    ///
    /// # Errors
    /// [`ErrorKind::BrokenPipe`] once the channel's write side is gone.
    pub fn send_line(&self, command: &str) -> std::io::Result<()> {
        let mut bytes = command.as_bytes().to_vec();
        bytes.push(b'\n');
        self.send_bytes(bytes)
    }

    /// Queue raw bytes for the tmux client's stdin (never blocks).
    ///
    /// # Errors
    /// [`ErrorKind::BrokenPipe`] once the channel's write side is gone.
    pub fn send_bytes(&self, bytes: Vec<u8>) -> std::io::Result<()> {
        lock_unpoisoned(&self.input_tx)
            .as_ref()
            .and_then(|tx| tx.send(bytes).ok())
            .ok_or_else(|| ErrorKind::BrokenPipe.into())
    }

    /// Resize the control client's grid (tmux sizes new windows to it).
    pub fn resize(&self, cols: u16, rows: u16) {
        if let Some(pty) = lock_unpoisoned(&self.pty).as_mut() {
            pty.on_resize(WindowSize {
                num_lines: rows.max(1),
                num_cols: cols.max(1),
                cell_width: 0,
                cell_height: 0,
            });
        }
    }

    /// The tmux client's process id (diagnostics + the reap test).
    #[must_use]
    pub const fn child_pid(&self) -> u32 {
        self.child_pid
    }

    /// A cloneable command-line handle onto this channel — what a mounted pane
    /// widget holds to route its typed input through [`commands::send_keys`]
    /// without owning the channel (TMUX-FC-3's pane-content mount seam).
    #[must_use]
    pub fn sink(&self) -> CommandSink {
        CommandSink {
            input_tx: Arc::clone(&self.input_tx),
        }
    }
}

/// A cloneable write-side handle onto a [`ControlChannel`].
///
/// One command line per call, honestly [`CommandSink::is_closed`] once the
/// channel dies. Held by each mounted pane's [`TmuxPaneIo`] so many widgets
/// share the one control channel.
#[derive(Clone)]
pub struct CommandSink {
    input_tx: Arc<Mutex<Option<Sender<Vec<u8>>>>>,
}

impl CommandSink {
    /// Write one control-mode command line (a trailing `\n` is added).
    ///
    /// # Errors
    /// [`ErrorKind::BrokenPipe`] once the channel's write side is gone.
    pub fn send_line(&self, command: &str) -> std::io::Result<()> {
        let mut bytes = command.as_bytes().to_vec();
        bytes.push(b'\n');
        lock_unpoisoned(&self.input_tx)
            .as_ref()
            .and_then(|tx| tx.send(bytes).ok())
            .ok_or_else(|| ErrorKind::BrokenPipe.into())
    }

    /// `true` once the channel's write side is gone (the tmux client exited).
    #[must_use]
    pub fn is_closed(&self) -> bool {
        lock_unpoisoned(&self.input_tx).is_none()
    }

    /// A sink wired to a raw queue instead of a live channel — the test seam
    /// (asserts the exact command lines a mounted pane emits).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn for_tests(tx: Sender<Vec<u8>>) -> Self {
        Self {
            input_tx: Arc::new(Mutex::new(Some(tx))),
        }
    }
}

impl Drop for ControlChannel {
    fn drop(&mut self) {
        drop(lock_unpoisoned(&self.input_tx).take());
        let pty = lock_unpoisoned(&self.pty).take();
        drop(pty);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

/// The PTY→queue pump: blocking-read the master, forward raw chunks, until EOF.
fn pump_raw(mut file: File, out_tx: &Sender<Vec<u8>>, output_closed: &AtomicBool) {
    let mut buf = [0_u8; CTRL_READ_CHUNK];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if out_tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    output_closed.store(true, Ordering::Release);
}

/// The queue→PTY pump: drain queued command/input bytes into the master.
fn pump_input(mut file: File, input_rx: &Receiver<Vec<u8>>) {
    while let Ok(chunk) = input_rx.recv() {
        if file.write_all(&chunk).is_err() {
            break;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The control-mode protocol parser.
// ─────────────────────────────────────────────────────────────────────────────

/// A framed command reply — the body between a `%begin` and its `%end`/`%error`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommandReply {
    /// The command number echoed by `%begin`/`%end` (correlates a reply to its
    /// command).
    pub number: u64,
    /// The flags integer from `%begin`/`%end`.
    pub flags: i64,
    /// The reply body lines (empty for a command with no output).
    pub lines: Vec<String>,
    /// `true` when the block closed with `%error` rather than `%end`.
    pub error: bool,
}

/// One control-mode notification (a `%`-line), or a framed [`CommandReply`].
///
/// The parser preserves any unrecognised `%`-line as [`Self::Other`] rather than
/// dropping or failing on it — new tmux notifications never break the stream.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Notification {
    /// A framed command reply (`%begin` … `%end`/`%error`).
    Reply(CommandReply),
    /// `%output %<pane> <data>` — the pane's output, already octal-unescaped.
    Output {
        /// The pane id (the `%N` number).
        pane: u32,
        /// The decoded (unescaped) output bytes.
        data: Vec<u8>,
    },
    /// `%window-add @<window>` — a window linked into the current session.
    WindowAdd {
        /// The window id (the `@N` number).
        window: u32,
    },
    /// `%window-close @<window>` — a window closed.
    WindowClose {
        /// The window id.
        window: u32,
    },
    /// `%window-renamed @<window> <name>`.
    WindowRenamed {
        /// The window id.
        window: u32,
        /// The new window name.
        name: String,
    },
    /// `%layout-change @<window> <layout> [<visible-layout> <flags>]`.
    LayoutChange {
        /// The window id.
        window: u32,
        /// The full layout string (checksum-prefixed).
        layout: String,
        /// The visible-layout string (newer tmux), if present. When the window
        /// is zoomed this is the zoomed arrangement (the one on screen), while
        /// `layout` stays the full pane set.
        visible: Option<String>,
        /// The window-flags token (newer tmux), if present — `Z` inside it means
        /// the window is zoomed.
        flags: Option<String>,
    },
    /// `%session-changed $<session> <name>` — the attached session changed.
    SessionChanged {
        /// The session id (the `$N` number).
        session: u32,
        /// The session name.
        name: String,
    },
    /// `%sessions-changed` — the set of sessions changed (no arguments).
    SessionsChanged,
    /// `%session-renamed [$<session>] <name>`.
    SessionRenamed {
        /// The session id, when the notification carries one.
        session: Option<u32>,
        /// The new session name.
        name: String,
    },
    /// `%unlinked-window-add @<window>` — a window not in the current session.
    UnlinkedWindowAdd {
        /// The window id.
        window: u32,
    },
    /// `%unlinked-window-close @<window>`.
    UnlinkedWindowClose {
        /// The window id.
        window: u32,
    },
    /// `%unlinked-window-renamed @<window> <name>`.
    UnlinkedWindowRenamed {
        /// The window id.
        window: u32,
        /// The new window name.
        name: String,
    },
    /// `%session-window-changed $<session> @<window>` — the session's current
    /// window changed (the round-trip echo of a `select-window`).
    SessionWindowChanged {
        /// The session id.
        session: u32,
        /// The now-current window id.
        window: u32,
    },
    /// `%window-pane-changed @<window> %<pane>` — the window's active pane.
    WindowPaneChanged {
        /// The window id.
        window: u32,
        /// The now-active pane id.
        pane: u32,
    },
    /// `%pane-mode-changed %<pane>` — a pane entered/left a mode (e.g. copy).
    PaneModeChanged {
        /// The pane id.
        pane: u32,
    },
    /// `%exit [<reason>]` — the control client is detaching / the server exiting.
    Exit {
        /// The optional exit reason.
        reason: Option<String>,
    },
    /// `%client-detached <client>`.
    ClientDetached {
        /// The client name.
        client: String,
    },
    /// Any other `%`-line, preserved verbatim (robustness — never a crash).
    Other(String),
}

/// The in-progress reply block between `%begin` and its terminator.
#[derive(Clone, Debug)]
struct PendingReply {
    number: u64,
    flags: i64,
    lines: Vec<String>,
}

/// The incremental control-mode protocol parser.
///
/// Robust to partial reads: [`Self::feed`] buffers bytes until a full
/// `\n`-terminated line and only then emits notifications, so a chunk that ends
/// mid-line — or delivers a line one byte at a time — yields the exact same
/// [`Notification`] sequence as one delivering whole lines.
#[derive(Default)]
pub struct Parser {
    buf: Vec<u8>,
    pending: Option<PendingReply>,
}

impl Parser {
    /// A fresh parser with no buffered bytes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a run of control-channel bytes, returning every notification that
    /// completed. Any trailing partial line is retained for the next call.
    #[must_use]
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Notification> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=nl).collect();
            line.pop(); // the '\n'
            if line.last() == Some(&b'\r') {
                line.pop(); // a '\r\n' terminator
            }
            self.process_line(&line, &mut out);
        }
        out
    }

    /// Process one complete line (no terminator).
    fn process_line(&mut self, line: &[u8], out: &mut Vec<Notification>) {
        // Inside a reply block, every line is body until the matching terminator.
        if self.pending.is_some() {
            let text = String::from_utf8_lossy(line);
            if text.starts_with("%end") || text.starts_with("%error") {
                let error = text.starts_with("%error");
                if let Some(p) = self.pending.take() {
                    out.push(Notification::Reply(CommandReply {
                        number: p.number,
                        flags: p.flags,
                        lines: p.lines,
                        error,
                    }));
                }
            } else if let Some(p) = self.pending.as_mut() {
                p.lines.push(text.into_owned());
            }
            return;
        }

        if line.first() != Some(&b'%') {
            // Control mode only sends `%`-lines outside a reply block; keep any
            // stray line rather than lose it.
            if !line.is_empty() {
                out.push(Notification::Other(
                    String::from_utf8_lossy(line).into_owned(),
                ));
            }
            return;
        }

        // `%output` carries binary data — decode it from the raw bytes so a
        // space or non-UTF-8 byte in the payload is never mis-split.
        if let Some(rest) = line.strip_prefix(b"%output ") {
            if let Some(sp) = rest.iter().position(|&b| b == b' ') {
                let pane_tok = String::from_utf8_lossy(&rest[..sp]);
                if let Some(pane) = parse_id(&pane_tok, '%') {
                    out.push(Notification::Output {
                        pane,
                        data: unescape_octal(&rest[sp + 1..]),
                    });
                    return;
                }
            }
            out.push(Notification::Other(
                String::from_utf8_lossy(line).into_owned(),
            ));
            return;
        }

        let text = String::from_utf8_lossy(line);
        out.push(self.parse_notification(&text));
    }

    /// Parse a non-`%output`, non-reply-body `%`-line into a [`Notification`].
    fn parse_notification(&mut self, line: &str) -> Notification {
        let (kw, args) = split_head(line);
        match kw {
            "%begin" => {
                let (number, flags) = parse_begin(args);
                self.pending = Some(PendingReply {
                    number,
                    flags,
                    lines: Vec::new(),
                });
                // `%begin` itself surfaces nothing; the completed reply does.
                Notification::Other(line.to_owned())
            }
            "%window-add" => id_note(args, '@', |window| Notification::WindowAdd { window }),
            "%window-close" => id_note(args, '@', |window| Notification::WindowClose { window }),
            "%window-renamed" => renamed(args, '@', |window, name| Notification::WindowRenamed {
                window,
                name,
            }),
            "%layout-change" => parse_layout_change(args),
            "%session-changed" => {
                let (id, name) = split_head(args);
                parse_id(id, '$').map_or_else(
                    || Notification::Other(line.to_owned()),
                    |session| Notification::SessionChanged {
                        session,
                        name: name.to_owned(),
                    },
                )
            }
            "%sessions-changed" => Notification::SessionsChanged,
            "%session-renamed" => {
                let (first, rest) = split_head(args);
                parse_id(first, '$').map_or_else(
                    || Notification::SessionRenamed {
                        session: None,
                        name: args.to_owned(),
                    },
                    |session| Notification::SessionRenamed {
                        session: Some(session),
                        name: rest.to_owned(),
                    },
                )
            }
            "%unlinked-window-add" => id_note(args, '@', |window| {
                Notification::UnlinkedWindowAdd { window }
            }),
            "%unlinked-window-close" => id_note(args, '@', |window| {
                Notification::UnlinkedWindowClose { window }
            }),
            "%unlinked-window-renamed" => renamed(args, '@', |window, name| {
                Notification::UnlinkedWindowRenamed { window, name }
            }),
            "%session-window-changed" => {
                let (sess_tok, win_tok) = split_head(args);
                match (parse_id(sess_tok, '$'), parse_id(win_tok.trim(), '@')) {
                    (Some(session), Some(window)) => {
                        Notification::SessionWindowChanged { session, window }
                    }
                    _ => Notification::Other(line.to_owned()),
                }
            }
            "%window-pane-changed" => {
                let (win_tok, pane_tok) = split_head(args);
                match (parse_id(win_tok, '@'), parse_id(pane_tok.trim(), '%')) {
                    (Some(window), Some(pane)) => Notification::WindowPaneChanged { window, pane },
                    _ => Notification::Other(line.to_owned()),
                }
            }
            "%pane-mode-changed" => {
                let (pane_tok, _) = split_head(args);
                parse_id(pane_tok, '%').map_or_else(
                    || Notification::Other(line.to_owned()),
                    |pane| Notification::PaneModeChanged { pane },
                )
            }
            "%exit" => Notification::Exit {
                reason: (!args.is_empty()).then(|| args.to_owned()),
            },
            "%client-detached" => Notification::ClientDetached {
                client: args.to_owned(),
            },
            _ => Notification::Other(line.to_owned()),
        }
    }
}

/// Split a line into its leading token and the (untrimmed) remainder.
fn split_head(line: &str) -> (&str, &str) {
    line.find(' ')
        .map_or((line, ""), |i| (&line[..i], &line[i + 1..]))
}

/// Parse `%begin`/`%end` args `<timestamp> <number> <flags>` → `(number, flags)`.
fn parse_begin(args: &str) -> (u64, i64) {
    let mut it = args.split_whitespace();
    let _ts = it.next();
    let number = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let flags = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (number, flags)
}

/// Build a single-id notification, or [`Notification::Other`] if the id is bad.
fn id_note(args: &str, sigil: char, make: impl Fn(u32) -> Notification) -> Notification {
    parse_id(args.trim(), sigil).map_or_else(|| Notification::Other(args.to_owned()), make)
}

/// Build an `<id> <name>` renamed notification (name keeps internal spaces).
fn renamed(args: &str, sigil: char, make: impl Fn(u32, String) -> Notification) -> Notification {
    let (id, name) = split_head(args);
    parse_id(id, sigil).map_or_else(
        || Notification::Other(args.to_owned()),
        |n| make(n, name.to_owned()),
    )
}

/// Parse `%layout-change` args: `@<window> <layout> [<visible> <flags>]`.
fn parse_layout_change(args: &str) -> Notification {
    let mut it = args.split_whitespace();
    let win = it.next();
    let layout = it.next();
    let visible = it.next().map(str::to_owned);
    let flags = it.next().map(str::to_owned);
    match (win.and_then(|w| parse_id(w, '@')), layout) {
        (Some(window), Some(layout)) => Notification::LayoutChange {
            window,
            layout: layout.to_owned(),
            visible,
            flags,
        },
        _ => Notification::Other(format!("%layout-change {args}")),
    }
}

/// Parse a sigil-prefixed id token (`%1`, `@0`, `$2`) into its number.
fn parse_id(tok: &str, sigil: char) -> Option<u32> {
    tok.strip_prefix(sigil).and_then(|n| n.parse().ok())
}

/// Decode tmux control-mode octal escaping (`\ooo`, three octal digits) back to
/// raw bytes. A backslash not followed by three octal digits is kept literal.
#[must_use]
fn unescape_octal(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\\' && i + 3 < data.len() {
            let d = &data[i + 1..=i + 3];
            if d.iter().all(|b| (b'0'..=b'7').contains(b)) {
                let val = u16::from(d[0] - b'0') * 64
                    + u16::from(d[1] - b'0') * 8
                    + u16::from(d[2] - b'0');
                if let Ok(byte) = u8::try_from(val) {
                    out.push(byte);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(data[i]);
        i += 1;
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// The tmux layout string → a pane tree.
// ─────────────────────────────────────────────────────────────────────────────

/// Which way a tmux layout container arranges its children.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutDir {
    /// `{...}` — children side by side (a vertical divider between them).
    LeftRight,
    /// `[...]` — children stacked (a horizontal divider between them).
    TopBottom,
}

/// What a [`Layout`] cell is: a single pane, or a split of child cells.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LayoutKind {
    /// A single pane, holding its tmux pane id.
    Pane(u32),
    /// A split into `>= 2` child cells in the given direction.
    Split(LayoutDir, Vec<Layout>),
}

/// One node of a parsed tmux layout string — a cell rectangle (in grid cells)
/// and its contents.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Layout {
    /// Cell-rectangle width (columns).
    pub width: u16,
    /// Cell-rectangle height (rows).
    pub height: u16,
    /// Cell-rectangle left offset within the window.
    pub x: u16,
    /// Cell-rectangle top offset within the window.
    pub y: u16,
    /// Whether this cell is a pane or a split.
    pub kind: LayoutKind,
}

/// Why a layout string failed to parse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayoutError {
    /// The string was empty or had no checksum separator.
    Malformed,
    /// A number, `x`, `,`, or bracket was expected but not found (byte offset).
    Unexpected(usize),
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed => write!(f, "malformed tmux layout string"),
            Self::Unexpected(at) => write!(f, "unexpected byte in tmux layout at offset {at}"),
        }
    }
}

impl std::error::Error for LayoutError {}

impl Layout {
    /// Every pane id in this subtree, in reading order.
    #[must_use]
    pub fn pane_ids(&self) -> Vec<u32> {
        let mut out = Vec::new();
        self.collect_pane_ids(&mut out);
        out
    }

    fn collect_pane_ids(&self, out: &mut Vec<u32>) {
        match &self.kind {
            LayoutKind::Pane(id) => out.push(*id),
            LayoutKind::Split(_, cells) => {
                for c in cells {
                    c.collect_pane_ids(out);
                }
            }
        }
    }

    /// Fold this layout into the native [`crate::splits::Pane`] binary tree,
    /// converting an n-ary tmux container into a right-leaning binary chain whose
    /// ratios preserve the cell proportions. Each pane id becomes the leaf's
    /// [`SessionId`], so a renderer maps a native leaf straight back to the tmux
    /// pane (and its [`TmuxModel::pane_terminal`]).
    #[must_use]
    pub fn to_pane_tree(&self) -> SplitPane {
        match &self.kind {
            LayoutKind::Pane(id) => SplitPane::Leaf(SessionId(u64::from(*id))),
            LayoutKind::Split(dir, cells) => {
                let split_dir = match dir {
                    LayoutDir::LeftRight => SplitDir::V,
                    LayoutDir::TopBottom => SplitDir::H,
                };
                fold_children(cells, split_dir, *dir)
            }
        }
    }
}

/// The size a child contributes along the split axis.
fn axis_size(cell: &Layout, dir: LayoutDir) -> f32 {
    match dir {
        LayoutDir::LeftRight => f32::from(cell.width),
        LayoutDir::TopBottom => f32::from(cell.height),
    }
}

/// Fold a slice of sibling cells (`>= 1`) into a right-leaning binary tree.
fn fold_children(cells: &[Layout], split: SplitDir, dir: LayoutDir) -> SplitPane {
    match cells {
        [] => SplitPane::Leaf(SessionId(0)), // unreachable: containers hold >= 2
        [only] => only.to_pane_tree(),
        [first, rest @ ..] => {
            let a_size = axis_size(first, dir);
            let rest_size: f32 = rest.iter().map(|c| axis_size(c, dir)).sum();
            let total = a_size + rest_size;
            let ratio = if total > 0.0 {
                clamp_ratio(a_size / total)
            } else {
                0.5
            };
            SplitPane::Split {
                dir: split,
                ratio,
                a: Box::new(first.to_pane_tree()),
                b: Box::new(fold_children(rest, split, dir)),
            }
        }
    }
}

/// Parse a tmux layout string (`<checksum>,<cell>`) into a [`Layout`] tree.
///
/// # Errors
/// [`LayoutError`] when the checksum separator is missing or a cell is malformed.
pub fn parse_layout(s: &str) -> Result<Layout, LayoutError> {
    // Strip the leading 4-hex-digit checksum and its comma.
    let comma = s.find(',').ok_or(LayoutError::Malformed)?;
    let body = &s.as_bytes()[comma + 1..];
    let mut cur = LayoutCursor { b: body, i: 0 };
    let layout = cur.cell()?;
    Ok(layout)
}

/// A byte cursor over a layout body (recursive-descent).
struct LayoutCursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl LayoutCursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn eat(&mut self, byte: u8) -> Result<(), LayoutError> {
        if self.peek() == Some(byte) {
            self.i += 1;
            Ok(())
        } else {
            Err(LayoutError::Unexpected(self.i))
        }
    }

    fn number(&mut self) -> Result<u32, LayoutError> {
        let start = self.i;
        let mut val: u32 = 0;
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() {
                val = val.saturating_mul(10).saturating_add(u32::from(b - b'0'));
                self.i += 1;
            } else {
                break;
            }
        }
        if self.i == start {
            Err(LayoutError::Unexpected(self.i))
        } else {
            Ok(val)
        }
    }

    /// Parse one cell: `WxH,X,Y` then a pane id, `{...}`, or `[...]`.
    fn cell(&mut self) -> Result<Layout, LayoutError> {
        let width = clamp_u16(self.number()?);
        self.eat(b'x')?;
        let height = clamp_u16(self.number()?);
        self.eat(b',')?;
        let x = clamp_u16(self.number()?);
        self.eat(b',')?;
        let y = clamp_u16(self.number()?);

        let kind = match self.peek() {
            Some(b'{') => {
                self.i += 1;
                let cells = self.cell_list()?;
                self.eat(b'}')?;
                LayoutKind::Split(LayoutDir::LeftRight, cells)
            }
            Some(b'[') => {
                self.i += 1;
                let cells = self.cell_list()?;
                self.eat(b']')?;
                LayoutKind::Split(LayoutDir::TopBottom, cells)
            }
            Some(b',') => {
                self.i += 1;
                LayoutKind::Pane(self.number()?)
            }
            _ => return Err(LayoutError::Unexpected(self.i)),
        };

        Ok(Layout {
            width,
            height,
            x,
            y,
            kind,
        })
    }

    /// Parse a comma-separated list of cells (at least one).
    fn cell_list(&mut self) -> Result<Vec<Layout>, LayoutError> {
        let mut cells = vec![self.cell()?];
        while self.peek() == Some(b',') {
            self.i += 1;
            cells.push(self.cell()?);
        }
        Ok(cells)
    }
}

/// Clamp a layout dimension to `u16` (grids never exceed it).
fn clamp_u16(n: u32) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

// ─────────────────────────────────────────────────────────────────────────────
// The live model: sessions → windows → panes.
// ─────────────────────────────────────────────────────────────────────────────

/// A tmux session in the model.
#[derive(Clone, Debug, Default)]
pub struct TmuxSession {
    name: String,
}

impl TmuxSession {
    /// The session name (as tmux reports it).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A tmux window in the model — mapped to one native tab.
#[derive(Clone, Debug, Default)]
pub struct TmuxWindow {
    name: String,
    session: Option<u32>,
    layout: Option<Layout>,
    /// The parsed visible layout, when it differs from the full one (a zoomed
    /// window shows only the zoomed pane while `layout` keeps the full set).
    visible: Option<Layout>,
    active_pane: Option<u32>,
    linked: bool,
    /// Whether the window is zoomed (`Z` in the `%layout-change` flags).
    zoomed: bool,
    /// The window's position in its session (`#{window_index}`), learned from a
    /// [`parse_window_order`] reply — tab-strip order, distinct from the id.
    index: Option<u32>,
}

impl TmuxWindow {
    /// The window name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The session this window belongs to, when known.
    #[must_use]
    pub const fn session(&self) -> Option<u32> {
        self.session
    }

    /// The window's current parsed layout, when a `%layout-change` has arrived.
    #[must_use]
    pub const fn layout(&self) -> Option<&Layout> {
        self.layout.as_ref()
    }

    /// The window's active pane, when tmux has reported one.
    #[must_use]
    pub const fn active_pane(&self) -> Option<u32> {
        self.active_pane
    }

    /// Whether this window is linked into the current session (vs. an unlinked
    /// window living only in another session).
    #[must_use]
    pub const fn is_linked(&self) -> bool {
        self.linked
    }

    /// Whether the window is zoomed (one pane temporarily fills it).
    #[must_use]
    pub const fn is_zoomed(&self) -> bool {
        self.zoomed
    }

    /// The window's position in its session, when a window-order reply has
    /// reported one.
    #[must_use]
    pub const fn index(&self) -> Option<u32> {
        self.index
    }
}

/// A tmux pane in the model — mapped to one native split leaf.
///
/// Owns the [`Terminal`] engine `%output` feeds and a [`crate::widget::TerminalWidget`]
/// renders. The engine is shared (`Arc<Mutex<_>>`) exactly as [`crate::pty::LocalPty`]
/// shares its engine, so the mount seam (TMUX-FC-2) hands the renderer this same
/// grid without copying.
pub struct TmuxPane {
    window: Option<u32>,
    title: String,
    in_mode: bool,
    width: u16,
    height: u16,
    terminal: Arc<Mutex<Terminal>>,
}

impl TmuxPane {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            window: None,
            title: String::new(),
            in_mode: false,
            width: cols,
            height: rows,
            terminal: Arc::new(Mutex::new(Terminal::with_default_scrollback(
                usize::from(cols.max(1)),
                usize::from(rows.max(1)),
            ))),
        }
    }

    /// The window this pane belongs to, when placed by a layout-change.
    #[must_use]
    pub const fn window(&self) -> Option<u32> {
        self.window
    }

    /// The pane title (empty until set).
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Whether the pane is currently in a tmux mode (e.g. copy-mode).
    #[must_use]
    pub const fn in_mode(&self) -> bool {
        self.in_mode
    }

    /// The pane's cell size from its last layout-change.
    #[must_use]
    pub const fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// The shared engine `%output` feeds — the grid a renderer paints (§6, the
    /// same [`Terminal`] a [`crate::widget::TerminalWidget`] renders).
    #[must_use]
    pub fn terminal(&self) -> Arc<Mutex<Terminal>> {
        Arc::clone(&self.terminal)
    }
}

/// The live model of a tmux server as seen over a control channel.
///
/// Reconciled purely from [`Notification`]s via [`Self::apply`] — the GUI never
/// mutates it directly; it issues a [`commands`] string and the resulting
/// `%`-event updates the model (the design's round-trip invariant).
#[derive(Default)]
pub struct TmuxModel {
    sessions: BTreeMap<u32, TmuxSession>,
    windows: BTreeMap<u32, TmuxWindow>,
    panes: BTreeMap<u32, TmuxPane>,
    current_session: Option<u32>,
    /// The session's current window (`%session-window-changed`).
    current_window: Option<u32>,
    exited: Option<String>,
}

impl TmuxModel {
    /// An empty model.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reconcile one notification into the model.
    pub fn apply(&mut self, note: Notification) {
        match note {
            Notification::Output { pane, data } => self.feed_pane(pane, &data),
            Notification::WindowAdd { window } => self.add_window(window, true),
            Notification::UnlinkedWindowAdd { window } => self.add_window(window, false),
            Notification::WindowClose { window } | Notification::UnlinkedWindowClose { window } => {
                self.close_window(window);
            }
            Notification::WindowRenamed { window, name }
            | Notification::UnlinkedWindowRenamed { window, name } => {
                self.windows.entry(window).or_default().name = name;
            }
            Notification::LayoutChange {
                window,
                layout,
                visible,
                flags,
            } => self.relayout(window, &layout, visible.as_deref(), flags.as_deref()),
            Notification::SessionChanged { session, name } => {
                self.current_session = Some(session);
                self.sessions.entry(session).or_default().name = name;
            }
            Notification::SessionWindowChanged { session, window } => {
                if self.current_session.is_none_or(|s| s == session) {
                    self.current_window = Some(window);
                }
            }
            Notification::SessionRenamed { session, name } => {
                let id = session.or(self.current_session);
                if let Some(id) = id {
                    self.sessions.entry(id).or_default().name = name;
                }
            }
            Notification::WindowPaneChanged { window, pane } => {
                self.windows.entry(window).or_default().active_pane = Some(pane);
            }
            Notification::PaneModeChanged { pane } => {
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.in_mode = !p.in_mode;
                }
            }
            Notification::Exit { reason } => {
                self.exited = Some(reason.unwrap_or_default());
            }
            // Markers + framing the model does not itself act on (session
            // enumeration + client bookkeeping land with TMUX-FC-3/7).
            Notification::Reply(_)
            | Notification::SessionsChanged
            | Notification::ClientDetached { .. }
            | Notification::Other(_) => {}
        }
    }

    fn feed_pane(&mut self, pane: u32, data: &[u8]) {
        let entry = self
            .panes
            .entry(pane)
            .or_insert_with(|| TmuxPane::new(DEFAULT_PANE_COLS, DEFAULT_PANE_ROWS));
        lock_unpoisoned(&entry.terminal).feed(data);
    }

    fn add_window(&mut self, window: u32, linked: bool) {
        let entry = self.windows.entry(window).or_default();
        entry.linked = linked;
        if entry.session.is_none() {
            entry.session = self.current_session;
        }
    }

    fn close_window(&mut self, window: u32) {
        self.windows.remove(&window);
        self.panes.retain(|_, p| p.window != Some(window));
        if self.current_window == Some(window) {
            self.current_window = None;
        }
    }

    /// Reconcile a window's pane-set + arrangement from a new layout string:
    /// create panes new to the window (sized to their cells), resize existing
    /// ones, and drop panes no longer present. The optional visible layout +
    /// flags carry the zoom truth: `Z` in the flags means the window is zoomed
    /// and the visible layout (the zoomed pane filling the window) is what's on
    /// screen, while the full `layout` keeps the whole pane set alive.
    fn relayout(&mut self, window: u32, layout: &str, visible: Option<&str>, flags: Option<&str>) {
        let Ok(parsed) = parse_layout(layout) else {
            return;
        };
        let ids = parsed.pane_ids();
        let zoomed = flags.is_some_and(|f| f.contains('Z'));
        let visible_parsed = visible.and_then(|v| parse_layout(v).ok());

        // Place + size each pane the layout mentions.
        collect_leaf_cells(&parsed, &mut |pane, w, h| {
            let entry = self
                .panes
                .entry(pane)
                .or_insert_with(|| TmuxPane::new(w, h));
            entry.window = Some(window);
            entry.width = w;
            entry.height = h;
            lock_unpoisoned(&entry.terminal).resize(usize::from(w.max(1)), usize::from(h.max(1)));
        });

        // A zoomed window paints its visible arrangement — size those panes (in
        // practice the one zoomed pane) to their on-screen cells instead.
        if zoomed {
            if let Some(vis) = visible_parsed.as_ref() {
                collect_leaf_cells(vis, &mut |pane, w, h| {
                    if let Some(entry) = self.panes.get_mut(&pane) {
                        entry.width = w;
                        entry.height = h;
                        lock_unpoisoned(&entry.terminal)
                            .resize(usize::from(w.max(1)), usize::from(h.max(1)));
                    }
                });
            }
        }

        // Drop panes that used to be in this window but the layout no longer has.
        self.panes
            .retain(|id, p| p.window != Some(window) || ids.contains(id));

        let entry = self.windows.entry(window).or_default();
        entry.layout = Some(parsed);
        entry.zoomed = zoomed;
        entry.visible = zoomed.then_some(visible_parsed).flatten();
        if entry.session.is_none() {
            entry.session = self.current_session;
        }
    }

    /// The current attached session id, when a `%session-changed` has arrived.
    #[must_use]
    pub const fn current_session(&self) -> Option<u32> {
        self.current_session
    }

    /// The session's current window, when a `%session-window-changed` has
    /// arrived (the window the view mounts; falls back to the first linked one).
    #[must_use]
    pub const fn current_window(&self) -> Option<u32> {
        self.current_window
    }

    /// Whether the control client has exited (with its reason, when given).
    #[must_use]
    pub fn exit_reason(&self) -> Option<&str> {
        self.exited.as_deref()
    }

    /// A session by id.
    #[must_use]
    pub fn session(&self, id: u32) -> Option<&TmuxSession> {
        self.sessions.get(&id)
    }

    /// Every known session id, ascending.
    #[must_use]
    pub fn session_ids(&self) -> Vec<u32> {
        self.sessions.keys().copied().collect()
    }

    /// A window by id.
    #[must_use]
    pub fn window(&self, id: u32) -> Option<&TmuxWindow> {
        self.windows.get(&id)
    }

    /// The linked windows (those forming the current session's tab strip), in
    /// session order — by `#{window_index}` once a window-order reply has
    /// landed ([`Self::apply_window_order`]), falling back to id order until
    /// then. This is what makes a `move-window` reorder visibly reconcile.
    #[must_use]
    pub fn windows_in_order(&self) -> Vec<u32> {
        let mut out: Vec<(u32, u32)> = self
            .windows
            .iter()
            .filter(|(_, w)| w.linked)
            .map(|(id, w)| (w.index.unwrap_or(*id), *id))
            .collect();
        out.sort_unstable();
        out.into_iter().map(|(_, id)| id).collect()
    }

    /// A pane by id.
    #[must_use]
    pub fn pane(&self, id: u32) -> Option<&TmuxPane> {
        self.panes.get(&id)
    }

    /// The panes belonging to a window, in the window's layout reading order
    /// (empty when the window has no layout yet).
    #[must_use]
    pub fn panes_of_window(&self, window: u32) -> Vec<u32> {
        self.windows
            .get(&window)
            .and_then(|w| w.layout.as_ref())
            .map(Layout::pane_ids)
            .unwrap_or_default()
    }

    /// The native split tree for a window — the mapping a renderer mounts as a
    /// [`crate::splits::SplitTerminal`] tab (leaves keyed by tmux pane id). A
    /// zoomed window yields its **visible** arrangement (the zoomed pane filling
    /// the window), exactly what tmux has on screen.
    #[must_use]
    pub fn window_tree(&self, window: u32) -> Option<SplitPane> {
        let win = self.windows.get(&window)?;
        if win.zoomed {
            if let Some(vis) = win.visible.as_ref() {
                return Some(vis.to_pane_tree());
            }
        }
        win.layout.as_ref().map(Layout::to_pane_tree)
    }

    /// Reconcile pane titles from a [`commands::list_pane_titles`] reply — the
    /// server truth after a `select-pane -T` rename (tmux emits no `%`-event
    /// for titles, so the reply is the round-trip's second half).
    pub fn apply_pane_titles(&mut self, titles: &[(u32, String)]) {
        for (pane, title) in titles {
            if let Some(p) = self.panes.get_mut(pane) {
                p.title.clone_from(title);
            }
        }
    }

    /// Reconcile the session's window order + membership from a
    /// [`commands::list_window_order`] reply: reported windows are the current
    /// session's tab strip (in `#{window_index}` order); linked windows the
    /// reply no longer mentions have left the session. Server truth — the
    /// reconcile after `move-window`/`break-pane`/`switch-client`.
    pub fn apply_window_order(&mut self, order: &[(u32, u32)]) {
        for (id, win) in &mut self.windows {
            match order.iter().find(|(w, _)| w == id) {
                Some((_, index)) => {
                    win.linked = true;
                    win.index = Some(*index);
                    if win.session.is_none() {
                        win.session = self.current_session;
                    }
                }
                None => win.linked = false,
            }
        }
        // Windows the model has not met yet (an attach to a pre-existing
        // session streams no `%window-add` for them).
        for (window, index) in order {
            let entry = self.windows.entry(*window).or_default();
            entry.linked = true;
            entry.index = Some(*index);
            if entry.session.is_none() {
                entry.session = self.current_session;
            }
        }
    }

    /// The shared engine of a pane — the grid `%output` feeds and a renderer
    /// paints (the "map each pane → a split leaf whose widget grid is fed the
    /// `%output`" seam).
    #[must_use]
    pub fn pane_terminal(&self, pane: u32) -> Option<Arc<Mutex<Terminal>>> {
        self.panes.get(&pane).map(TmuxPane::terminal)
    }
}

/// Walk a layout, invoking `sink(pane_id, width, height)` for each pane leaf.
fn collect_leaf_cells(layout: &Layout, sink: &mut impl FnMut(u32, u16, u16)) {
    match &layout.kind {
        LayoutKind::Pane(id) => sink(*id, layout.width, layout.height),
        LayoutKind::Split(_, cells) => {
            for c in cells {
                collect_leaf_cells(c, sink);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The command sink: GUI intent → tmux command string.
// ─────────────────────────────────────────────────────────────────────────────

/// The four directions a `resize-pane` can grow a pane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResizeDir {
    /// Grow toward the left (`-L`).
    Left,
    /// Grow toward the right (`-R`).
    Right,
    /// Grow upward (`-U`).
    Up,
    /// Grow downward (`-D`).
    Down,
}

/// Pure builders for the tmux commands the GUI writes to the control channel.
///
/// Each returns the exact command line; the resulting `%`-event is what
/// reconciles [`TmuxModel`] (never a direct native-tree mutation).
pub mod commands {
    use super::ResizeDir;
    use crate::splits::SplitDir;

    /// `select-pane -t %<pane>`.
    #[must_use]
    pub fn select_pane(pane: u32) -> String {
        format!("select-pane -t %{pane}")
    }

    /// `select-window -t @<window>`.
    #[must_use]
    pub fn select_window(window: u32) -> String {
        format!("select-window -t @{window}")
    }

    /// Split a pane. A native [`SplitDir::V`] (children side by side) is tmux's
    /// `-h`; [`SplitDir::H`] (stacked) is tmux's `-v`.
    #[must_use]
    pub fn split_window(pane: u32, dir: SplitDir) -> String {
        let flag = match dir {
            SplitDir::V => "-h",
            SplitDir::H => "-v",
        };
        format!("split-window -t %{pane} {flag}")
    }

    /// `kill-pane -t %<pane>`.
    #[must_use]
    pub fn kill_pane(pane: u32) -> String {
        format!("kill-pane -t %{pane}")
    }

    /// `kill-window -t @<window>`.
    #[must_use]
    pub fn kill_window(window: u32) -> String {
        format!("kill-window -t @{window}")
    }

    /// `new-window`.
    #[must_use]
    pub fn new_window() -> String {
        "new-window".to_owned()
    }

    /// `rename-window -t @<window> <name>` (the name safely tmux-quoted).
    #[must_use]
    pub fn rename_window(window: u32, name: &str) -> String {
        format!("rename-window -t @{window} {}", quote(name))
    }

    /// Rename a pane's title: `select-pane -t %<pane> -T <title>` (tmux-quoted).
    ///
    /// tmux emits no `%`-event for titles — [`super::TmuxController`] follows up
    /// with [`list_pane_titles`] so the reply reconciles the model.
    #[must_use]
    pub fn rename_pane(pane: u32, title: &str) -> String {
        format!("select-pane -t %{pane} -T {}", quote(title))
    }

    /// Toggle a pane's zoom: `resize-pane -t %<pane> -Z` (the `%layout-change`
    /// flags carry the `Z` truth back).
    #[must_use]
    pub fn zoom_pane(pane: u32) -> String {
        format!("resize-pane -t %{pane} -Z")
    }

    /// Break a pane out into its own window: `break-pane -s %<pane>`
    /// (`%window-add` + `%layout-change` reconcile).
    #[must_use]
    pub fn break_pane(pane: u32) -> String {
        format!("break-pane -s %{pane}")
    }

    /// Join (move) a pane into another window: `join-pane -h -s %<src> -t @<dst>`
    /// — a native [`SplitDir::V`] (side by side) is tmux's `-h`, [`SplitDir::H`]
    /// (stacked) its `-v`, matching [`split_window`].
    #[must_use]
    pub fn join_pane(src: u32, dst_window: u32, dir: SplitDir) -> String {
        let flag = match dir {
            SplitDir::V => "-h",
            SplitDir::H => "-v",
        };
        format!("join-pane {flag} -s %{src} -t @{dst_window}")
    }

    /// Swap two panes in place: `swap-pane -d -s %<a> -t %<b>` (`-d` keeps the
    /// active pane where the user is looking).
    #[must_use]
    pub fn swap_panes(a: u32, b: u32) -> String {
        format!("swap-pane -d -s %{a} -t %{b}")
    }

    /// Reorder: move a window immediately **before** another
    /// (`move-window -b -s @<src> -t @<dst>`) — the drag-reorder drop's command.
    #[must_use]
    pub fn move_window_before(src: u32, dst: u32) -> String {
        format!("move-window -b -s @{src} -t @{dst}")
    }

    /// Reorder: move a window immediately **after** another
    /// (`move-window -a -s @<src> -t @<dst>`) — the drop past the last tab.
    #[must_use]
    pub fn move_window_after(src: u32, dst: u32) -> String {
        format!("move-window -a -s @{src} -t @{dst}")
    }

    /// Set a pane's exact width in cells: `resize-pane -t %<pane> -x <cols>` —
    /// the vertical-divider drag's command (tmux moves the shared boundary).
    #[must_use]
    pub fn resize_pane_width(pane: u32, cols: u16) -> String {
        format!("resize-pane -t %{pane} -x {cols}")
    }

    /// Set a pane's exact height in cells: `resize-pane -t %<pane> -y <rows>` —
    /// the horizontal-divider drag's command.
    #[must_use]
    pub fn resize_pane_height(pane: u32, rows: u16) -> String {
        format!("resize-pane -t %{pane} -y {rows}")
    }

    /// Report the control client's grid size: `refresh-client -C <cols>x<rows>`.
    ///
    /// tmux lays windows out to the attached client's size, so the mounted view
    /// sends this when its rect's cell grid changes — the `%layout-change` that
    /// follows resizes every pane engine to what actually fits on screen.
    #[must_use]
    pub fn refresh_client_size(cols: u16, rows: u16) -> String {
        format!("refresh-client -C {cols}x{rows}")
    }

    /// Enumerate the current session's panes with their titles.
    ///
    /// Each line is tagged [`super::PANE_TITLE_TAG`] so the reply is
    /// self-identifying (never mistaken for a session list). Parsed by
    /// [`super::parse_pane_titles`].
    #[must_use]
    pub fn list_pane_titles() -> String {
        format!(
            "list-panes -s -F '{}\t#{{pane_id}}\t#{{pane_title}}'",
            super::PANE_TITLE_TAG
        )
    }

    /// Enumerate the current session's windows in index order, each line tagged
    /// [`super::WINDOW_ORDER_TAG`]. Parsed by [`super::parse_window_order`] —
    /// the tab-strip order truth after a reorder/break/switch.
    #[must_use]
    pub fn list_window_order() -> String {
        format!(
            "list-windows -F '{}\t#{{window_id}}\t#{{window_index}}'",
            super::WINDOW_ORDER_TAG
        )
    }

    /// Resize a pane to an exact cell size: `resize-pane -t %<pane> -x <c> -y <r>`.
    #[must_use]
    pub fn resize_pane_to(pane: u32, cols: u16, rows: u16) -> String {
        format!("resize-pane -t %{pane} -x {cols} -y {rows}")
    }

    /// Resize a pane by `cells` in a direction: `resize-pane -t %<pane> -L 3`.
    #[must_use]
    pub fn resize_pane(pane: u32, dir: ResizeDir, cells: u16) -> String {
        let flag = match dir {
            ResizeDir::Left => "-L",
            ResizeDir::Right => "-R",
            ResizeDir::Up => "-U",
            ResizeDir::Down => "-D",
        };
        format!("resize-pane -t %{pane} {flag} {cells}")
    }

    /// Send raw key bytes to a pane, hex-encoded so any byte is safe:
    /// `send-keys -t %<pane> -H 1b 5b 41`.
    #[must_use]
    pub fn send_keys(pane: u32, bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        let mut out = format!("send-keys -t %{pane} -H");
        for b in bytes {
            // Writing to a `String` is infallible.
            let _ = write!(out, " {b:02x}");
        }
        out
    }

    // ── session-level ops (TMUX-FC-2) ────────────────────────────────────────
    // Create / attach / detach / kill / rename + enumerate, each a `%`-event
    // round-trip: the command lands, tmux emits `%session-changed` /
    // `%sessions-changed` / `%exit`, and [`super::TmuxModel`] reconciles from it.

    /// Create a new session and switch this control client onto it:
    /// `new-session -s <name>` (control mode switches the client, emitting
    /// `%session-changed`; the name is tmux-quoted).
    #[must_use]
    pub fn new_session(name: &str) -> String {
        format!("new-session -s {}", quote(name))
    }

    /// Re-attach a detached (or other) session onto this control client.
    ///
    /// `switch-client -t <target>` — the control-mode way to move the one client
    /// between sessions without a second attach (emits `%session-changed`).
    #[must_use]
    pub fn attach_session(target: &str) -> String {
        format!("switch-client -t {}", quote(target))
    }

    /// Detach this control client: `detach-client`. The session keeps running on
    /// the node (design lock #6); the channel then sees `%exit`.
    #[must_use]
    pub fn detach_client() -> String {
        "detach-client".to_owned()
    }

    /// Kill a session outright: `kill-session -t <target>` (name tmux-quoted).
    #[must_use]
    pub fn kill_session(target: &str) -> String {
        format!("kill-session -t {}", quote(target))
    }

    /// Rename a session: `rename-session -t <target> <name>` (both tmux-quoted).
    #[must_use]
    pub fn rename_session(target: &str, name: &str) -> String {
        format!("rename-session -t {} {}", quote(target), quote(name))
    }

    /// Enumerate **all** sessions (attached AND detached) — the picker's source.
    ///
    /// Emits tab-separated `name<TAB>attached<TAB>windows` lines in the command
    /// reply. The `\t` separator survives session names with spaces
    /// (space-splitting would not). Parsed by [`super::parse_session_list`], which
    /// must stay in step with this format string.
    #[must_use]
    pub fn list_sessions() -> String {
        "list-sessions -F '#{session_name}\t#{session_attached}\t#{session_windows}'".to_owned()
    }

    /// tmux-quote a string: single-quote wrap, escaping any internal quote.
    fn quote(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('\'');
        for ch in s.chars() {
            if ch == '\'' {
                out.push_str("'\\''");
            } else {
                out.push(ch);
            }
        }
        out.push('\'');
        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The session enumeration: `list-sessions` → all sessions (attached + detached).
// ─────────────────────────────────────────────────────────────────────────────

/// One session as `list-sessions` reports it — the picker's row (TMUX-FC-2).
///
/// Unlike the [`TmuxModel`], which only fully knows the *attached* session's
/// windows/panes (control mode streams that one), this carries every session on
/// the server including the **detached** ones, so a detached session can be
/// picked and re-attached ([`commands::attach_session`]).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SessionInfo {
    /// The session name (the `switch-client`/`kill-session` target).
    pub name: String,
    /// Whether a client is currently attached (`#{session_attached}` > 0).
    pub attached: bool,
    /// How many windows the session holds (`#{session_windows}`).
    pub windows: u32,
}

/// Parse the reply body of [`commands::list_sessions`] into [`SessionInfo`]s.
///
/// Each line is `name<TAB>attached<TAB>windows`; a line without at least the
/// name+attached fields, or an empty name, is skipped rather than faked. Kept in
/// step with the `-F` format string [`commands::list_sessions`] emits.
#[must_use]
pub fn parse_session_list(lines: &[String]) -> Vec<SessionInfo> {
    lines
        .iter()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?.trim();
            if name.is_empty() {
                return None;
            }
            let attached = fields.next()?.trim().parse::<u32>().ok()? > 0;
            let windows = fields
                .next()
                .and_then(|f| f.trim().parse::<u32>().ok())
                .unwrap_or(0);
            Some(SessionInfo {
                name: name.to_owned(),
                attached,
                windows,
            })
        })
        .collect()
}

/// The self-identifying first field of a [`commands::list_pane_titles`] reply
/// line.
///
/// The tag is what routes a reply to its parser: replies carry no command
/// echo in control mode, and shape-sniffing alone could mistake one list for
/// another (a numeric pane title parses like a session row).
pub const PANE_TITLE_TAG: &str = "pane_title";

/// The self-identifying first field of a [`commands::list_window_order`] reply
/// line (see [`PANE_TITLE_TAG`]).
pub const WINDOW_ORDER_TAG: &str = "window_order";

/// Parse a [`commands::list_pane_titles`] reply body: tagged
/// `pane_title<TAB>%<pane><TAB><title>` lines → `(pane, title)` pairs.
///
/// A line without the tag or a `%`-id is skipped (it belongs to some other
/// reply). A title keeps any internal tabs.
#[must_use]
pub fn parse_pane_titles(lines: &[String]) -> Vec<(u32, String)> {
    lines
        .iter()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            if fields.next()? != PANE_TITLE_TAG {
                return None;
            }
            let pane = parse_id(fields.next()?.trim(), '%')?;
            Some((pane, fields.next().unwrap_or("").to_owned()))
        })
        .collect()
}

/// Parse a [`commands::list_window_order`] reply body: tagged
/// `window_order<TAB>@<window><TAB><index>` lines → `(window, index)` pairs in
/// reply (= session) order. Untagged/malformed lines are skipped.
#[must_use]
pub fn parse_window_order(lines: &[String]) -> Vec<(u32, u32)> {
    lines
        .iter()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            if fields.next()? != WINDOW_ORDER_TAG {
                return None;
            }
            let window = parse_id(fields.next()?.trim(), '@')?;
            let index = fields.next()?.trim().parse().ok()?;
            Some((window, index))
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Divider drag → `resize-pane`: the native-tree path mapped back to tmux cells.
// ─────────────────────────────────────────────────────────────────────────────

/// The `resize-pane` a divider drag resolves to.
///
/// Set `pane`'s size along the container's axis to `cells`
/// ([`commands::resize_pane_width`] for a [`LayoutDir::LeftRight`] container,
/// `_height` for [`LayoutDir::TopBottom`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PaneResize {
    /// The pane whose cell size carries the boundary (a leaf of the divider's
    /// first side spanning that side's full extent along the axis).
    pub pane: u32,
    /// Which axis the container splits on (→ `-x` vs `-y`).
    pub dir: LayoutDir,
    /// The new size in cells along that axis.
    pub cells: u16,
}

/// Resolve a dragged native divider back to the tmux `resize-pane` that moves
/// the same boundary.
///
/// The native tree ([`Layout::to_pane_tree`]) folds each n-ary tmux container
/// into a right-leaning binary chain, so the divider at [`NodePath`] `path`
/// separates one container child from its remaining siblings. This walks the
/// **tmux** layout in the same fold order, converts the dragged `ratio` into
/// cells along the container's axis, and picks the first-side leaf whose extent
/// spans that side (resizing it moves exactly this boundary). `None` when the
/// path doesn't address a divider or the span is too small to move.
#[must_use]
pub fn resize_for_divider(layout: &Layout, path: NodePath, ratio: f32) -> Option<PaneResize> {
    resize_in_cell(layout, NodePath::ROOT, path, ratio)
}

/// Descend into a layout cell looking for the divider at `target`.
fn resize_in_cell(
    cell: &Layout,
    cur: NodePath,
    target: NodePath,
    ratio: f32,
) -> Option<PaneResize> {
    match &cell.kind {
        LayoutKind::Pane(_) => None,
        LayoutKind::Split(dir, cells) => resize_in_chain(cells, *dir, cur, target, ratio),
    }
}

/// Descend a sibling chain exactly as [`fold_children`] built it: the node at
/// `cur` separates `cells[0]` (child a) from the rest (child b).
fn resize_in_chain(
    cells: &[Layout],
    dir: LayoutDir,
    cur: NodePath,
    target: NodePath,
    ratio: f32,
) -> Option<PaneResize> {
    match cells {
        [] => None,
        [only] => resize_in_cell(only, cur, target, ratio),
        [first, rest @ ..] => {
            if cur == target {
                let total =
                    axis_size(first, dir) + rest.iter().map(|c| axis_size(c, dir)).sum::<f32>();
                if total < 2.0 {
                    return None;
                }
                let cells_f = (ratio * total).round().clamp(1.0, total - 1.0);
                // Bounded to 1..=total-1 with total ≤ u16::MAX sums, so exact.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let cells_new = cells_f as u16;
                Some(PaneResize {
                    pane: boundary_leaf(first, dir)?,
                    dir,
                    cells: cells_new,
                })
            } else {
                resize_in_cell(first, cur.child_a(), target, ratio)
                    .or_else(|| resize_in_chain(rest, dir, cur.child_b(), target, ratio))
            }
        }
    }
}

/// A leaf of `cell` whose extent along `dir`'s axis spans the whole cell, so
/// `resize-pane`-ing it to N cells moves the cell's own boundary. A cross-axis
/// child always spans its parent, so descend the first; a same-axis child (a
/// denormalised nesting) only partially spans, so descend the one nearest the
/// boundary (the last).
fn boundary_leaf(cell: &Layout, dir: LayoutDir) -> Option<u32> {
    match &cell.kind {
        LayoutKind::Pane(id) => Some(*id),
        LayoutKind::Split(inner, cells) => {
            if *inner == dir {
                boundary_leaf(cells.last()?, dir)
            } else {
                boundary_leaf(cells.first()?, dir)
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The pane-content mount backing: a tmux pane behind the TERM-3 widget.
// ─────────────────────────────────────────────────────────────────────────────

/// The [`crate::session::Session::Tmux`] backing: one mounted tmux pane.
///
/// The shared engine `%output` feeds (rendered by the one TERM-3 grid, §6) plus
/// a [`CommandSink`] its typed input rides through [`commands::send_keys`]. The
/// round-trip discipline in widget form: input goes to tmux, never straight to
/// the grid; the grid changes only when `%output` arrives.
pub struct TmuxPaneIo {
    pane: u32,
    terminal: Arc<Mutex<Terminal>>,
    sink: CommandSink,
}

impl TmuxPaneIo {
    /// Bind pane `pane`'s shared engine to the control channel behind `sink`.
    #[must_use]
    pub const fn new(pane: u32, terminal: Arc<Mutex<Terminal>>, sink: CommandSink) -> Self {
        Self {
            pane,
            terminal,
            sink,
        }
    }

    /// The tmux pane id this mount drives.
    #[must_use]
    pub const fn pane(&self) -> u32 {
        self.pane
    }

    /// Route typed bytes to the pane as a `send-keys` command line.
    ///
    /// # Errors
    /// [`std::io::ErrorKind::BrokenPipe`] once the control channel is gone.
    pub fn send_input(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.sink.send_line(&commands::send_keys(self.pane, bytes))
    }

    /// Run `f` against the pane's engine (the render-agnostic snapshot source).
    pub fn with_terminal<R>(&self, f: impl FnOnce(&Terminal) -> R) -> R {
        f(&lock_unpoisoned(&self.terminal))
    }

    /// `true` once the control channel has died — the pane can't move again.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.sink.is_closed()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The controller: live channel + parser + model, with an honest status.
// ─────────────────────────────────────────────────────────────────────────────

/// The honest connection state of a [`TmuxController`] (§7 — no fake attach).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Status {
    /// Spawned; awaiting the first control-mode traffic.
    Connecting,
    /// Attached — control traffic is flowing.
    Attached,
    /// The control client could not start or `-CC` failed (with the reason).
    Error(String),
    /// The control client exited cleanly (`%exit`, with its reason if any).
    Exited(String),
}

/// A live tmux control-mode connection: the [`ControlChannel`] + the [`Parser`]
/// + the [`TmuxModel`] it reconciles, plus an honest [`Status`].
pub struct TmuxController {
    channel: Option<ControlChannel>,
    parser: Parser,
    model: TmuxModel,
    status: Status,
    /// Every session on the server (attached + detached), from the most recent
    /// [`Self::request_sessions`] reply — the TMUX-FC-2 picker's source. The
    /// model itself only fully knows the attached session's windows/panes.
    all_sessions: Vec<SessionInfo>,
}

impl TmuxController {
    /// Connect a control channel per `launch`. A refused spawn (no `tmux`, etc.)
    /// yields a [`Status::Error`] controller — never a fabricated attach.
    #[must_use]
    pub fn connect(launch: &TmuxLaunch) -> Self {
        match ControlChannel::spawn(launch) {
            Ok(channel) => Self {
                channel: Some(channel),
                parser: Parser::new(),
                model: TmuxModel::new(),
                status: Status::Connecting,
                all_sessions: Vec::new(),
            },
            Err(err) => Self {
                channel: None,
                parser: Parser::new(),
                model: TmuxModel::new(),
                status: Status::Error(format!("could not start tmux -CC: {err}")),
                all_sessions: Vec::new(),
            },
        }
    }

    /// Drain any pending control traffic into the model — call once per frame.
    pub fn pump(&mut self) {
        let (chunks, closed) = self.channel.as_ref().map_or_else(
            || (Vec::new(), false),
            |channel| {
                let mut chunks = Vec::new();
                while let Some(chunk) = channel.try_recv() {
                    chunks.push(chunk);
                }
                (chunks, channel.is_closed())
            },
        );

        let got = !chunks.is_empty();
        let mut refresh = false;
        for chunk in chunks {
            for note in self.parser.feed(&chunk) {
                refresh |= self.absorb(note);
            }
        }

        if got && self.status == Status::Connecting {
            self.status = Status::Attached;
        }
        if closed && self.status == Status::Connecting {
            self.status = Status::Error("tmux -CC exited before attaching".to_owned());
        }
        // The window set / session just changed: converge the tab-strip order +
        // membership + pane titles from server truth (once per pump, not per
        // notification, so an attach burst asks once).
        if refresh {
            let _ = self.request_window_order();
            let _ = self.request_pane_titles();
        }
    }

    /// Fold one notification into the controller + model. Returns `true` when
    /// the window set changed (the caller then re-asks for the order truth).
    fn absorb(&mut self, note: Notification) -> bool {
        let mut refresh = false;
        match &note {
            Notification::Exit { reason } => {
                self.status = Status::Exited(reason.clone().unwrap_or_default());
            }
            // Route a reply body to its parser. The tagged lists
            // (`list-pane-titles` / `list-window-order`) identify themselves;
            // the remaining non-empty list shape is `list-sessions`
            // (`name<TAB>attached<TAB>windows` — other ops reply empty), cached
            // for the picker. An empty/other reply leaves prior state intact.
            Notification::Reply(reply) if !reply.error => {
                let titles = parse_pane_titles(&reply.lines);
                if !titles.is_empty() {
                    self.model.apply_pane_titles(&titles);
                }
                let order = parse_window_order(&reply.lines);
                if !order.is_empty() {
                    self.model.apply_window_order(&order);
                }
                if titles.is_empty() && order.is_empty() {
                    let sessions = parse_session_list(&reply.lines);
                    if !sessions.is_empty() {
                        self.all_sessions = sessions;
                    }
                }
            }
            // The window set or session changed — the order/membership truth
            // (`#{window_index}` etc.) is not in the notification itself.
            Notification::WindowAdd { .. }
            | Notification::WindowClose { .. }
            | Notification::UnlinkedWindowAdd { .. }
            | Notification::UnlinkedWindowClose { .. }
            | Notification::SessionChanged { .. } => refresh = true,
            _ => {}
        }
        self.model.apply(note);
        refresh
    }

    /// Write a raw tmux command line (see [`commands`]).
    ///
    /// # Errors
    /// [`ErrorKind::BrokenPipe`] once the channel is gone (or never connected).
    pub fn send(&self, command: &str) -> std::io::Result<()> {
        self.channel.as_ref().map_or_else(
            || Err(ErrorKind::BrokenPipe.into()),
            |channel| channel.send_line(command),
        )
    }

    /// Ask the server to enumerate every session ([`commands::list_sessions`]);
    /// the reply lands in [`Self::all_sessions`] on a later [`Self::pump`].
    ///
    /// # Errors
    /// [`ErrorKind::BrokenPipe`] once the channel is gone (or never connected).
    pub fn request_sessions(&self) -> std::io::Result<()> {
        self.send(&commands::list_sessions())
    }

    /// Ask for the session's window order + membership
    /// ([`commands::list_window_order`]); the tagged reply reconciles the model
    /// on a later [`Self::pump`].
    ///
    /// # Errors
    /// [`ErrorKind::BrokenPipe`] once the channel is gone (or never connected).
    pub fn request_window_order(&self) -> std::io::Result<()> {
        self.send(&commands::list_window_order())
    }

    /// Ask for the session's pane titles ([`commands::list_pane_titles`]); the
    /// tagged reply reconciles the model on a later [`Self::pump`].
    ///
    /// # Errors
    /// [`ErrorKind::BrokenPipe`] once the channel is gone (or never connected).
    pub fn request_pane_titles(&self) -> std::io::Result<()> {
        self.send(&commands::list_pane_titles())
    }

    /// A cloneable command handle onto the live channel — what each mounted
    /// pane's [`TmuxPaneIo`] holds. `None` when no channel ever connected.
    #[must_use]
    pub fn sink(&self) -> Option<CommandSink> {
        self.channel.as_ref().map(ControlChannel::sink)
    }

    /// Every session on the server (attached + detached) from the last
    /// [`Self::request_sessions`] reply — the picker's rows (empty until asked).
    #[must_use]
    pub fn all_sessions(&self) -> &[SessionInfo] {
        &self.all_sessions
    }

    /// The honest connection status.
    #[must_use]
    pub const fn status(&self) -> &Status {
        &self.status
    }

    /// The live model (sessions → windows → panes).
    #[must_use]
    pub const fn model(&self) -> &TmuxModel {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    // ── the incremental protocol parser ──────────────────────────────────────

    /// A recorded control-mode stream covering each notification kind. Every
    /// `\n` is a real line terminator; `%output` payloads carry octal escapes.
    const RECORDED: &[u8] = b"\
%session-changed $0 main\n\
%window-add @0\n\
%layout-change @0 bd41,80x24,0,0,0 bd41,80x24,0,0,0 *\n\
%output %0 hello\\015\\012\n\
%window-renamed @0 editor\n\
%begin 1700000000 2 1\n\
one\n\
two\n\
%end 1700000000 2 1\n\
%window-pane-changed @0 %0\n\
%pane-mode-changed %0\n\
%unlinked-window-add @9\n\
%unlinked-window-renamed @9 detached\n\
%unlinked-window-close @9\n\
%window-close @0\n\
%begin 1700000001 3 0\n\
boom\n\
%error 1700000001 3 0\n\
%exit shutting down\n";

    fn parse_all(bytes: &[u8]) -> Vec<Notification> {
        let mut p = Parser::new();
        // Filter the empty `%begin` sentinels the parser emits as `Other` for the
        // begin line — they carry no model meaning and clutter the assertions.
        p.feed(bytes)
            .into_iter()
            .filter(|n| !matches!(n, Notification::Other(s) if s.starts_with("%begin")))
            .collect()
    }

    #[test]
    fn parses_each_notification_kind() {
        let notes = parse_all(RECORDED);
        assert_eq!(
            notes,
            vec![
                Notification::SessionChanged {
                    session: 0,
                    name: "main".to_owned()
                },
                Notification::WindowAdd { window: 0 },
                Notification::LayoutChange {
                    window: 0,
                    layout: "bd41,80x24,0,0,0".to_owned(),
                    visible: Some("bd41,80x24,0,0,0".to_owned()),
                    flags: Some("*".to_owned()),
                },
                Notification::Output {
                    pane: 0,
                    data: b"hello\r\n".to_vec(),
                },
                Notification::WindowRenamed {
                    window: 0,
                    name: "editor".to_owned()
                },
                Notification::Reply(CommandReply {
                    number: 2,
                    flags: 1,
                    lines: vec!["one".to_owned(), "two".to_owned()],
                    error: false,
                }),
                Notification::WindowPaneChanged { window: 0, pane: 0 },
                Notification::PaneModeChanged { pane: 0 },
                Notification::UnlinkedWindowAdd { window: 9 },
                Notification::UnlinkedWindowRenamed {
                    window: 9,
                    name: "detached".to_owned()
                },
                Notification::UnlinkedWindowClose { window: 9 },
                Notification::WindowClose { window: 0 },
                Notification::Reply(CommandReply {
                    number: 3,
                    flags: 0,
                    lines: vec!["boom".to_owned()],
                    error: true,
                }),
                Notification::Exit {
                    reason: Some("shutting down".to_owned())
                },
            ]
        );
    }

    #[test]
    fn partial_reads_yield_the_same_notifications() {
        // Feed the whole stream one byte at a time — a full line only completes
        // when its `\n` arrives, so the sequence must match the whole-buffer feed.
        let mut p = Parser::new();
        let mut notes = Vec::new();
        for b in RECORDED {
            notes.extend(p.feed(&[*b]));
        }
        let one_shot = {
            let mut q = Parser::new();
            q.feed(RECORDED)
        };
        assert_eq!(notes, one_shot);
    }

    #[test]
    fn output_across_a_chunk_boundary_is_buffered() {
        let mut p = Parser::new();
        // The line is split mid-payload: nothing emits until the `\n`.
        assert!(p.feed(b"%output %1 abc").is_empty());
        let notes = p.feed(b"def\n");
        assert_eq!(
            notes,
            vec![Notification::Output {
                pane: 1,
                data: b"abcdef".to_vec(),
            }]
        );
    }

    #[test]
    fn octal_unescape_decodes_control_bytes_and_backslash() {
        // \033 = ESC, \015\012 = CRLF, \134 = backslash; printables pass through.
        assert_eq!(unescape_octal(b"a\\033b"), b"a\x1bb");
        assert_eq!(unescape_octal(b"x\\015\\012"), b"x\r\n");
        assert_eq!(unescape_octal(b"c\\134d"), b"c\\d");
        // A lone trailing backslash (not 3 octal digits) stays literal.
        assert_eq!(unescape_octal(b"end\\"), b"end\\");
        assert_eq!(unescape_octal(b"\\9zz"), b"\\9zz");
    }

    #[test]
    fn unknown_percent_line_is_preserved_not_dropped() {
        let mut p = Parser::new();
        let notes = p.feed(b"%some-future-thing @1 stuff\n");
        assert_eq!(
            notes,
            vec![Notification::Other(
                "%some-future-thing @1 stuff".to_owned()
            )]
        );
    }

    // ── the layout string → pane tree ────────────────────────────────────────

    #[test]
    fn single_pane_layout_parses_to_a_leaf() {
        let l = parse_layout("bd41,80x24,0,0,0").expect("single-pane layout");
        assert_eq!(l.width, 80);
        assert_eq!(l.height, 24);
        assert_eq!(l.kind, LayoutKind::Pane(0));
        assert_eq!(l.pane_ids(), vec![0]);
        assert_eq!(l.to_pane_tree(), SplitPane::Leaf(SessionId(0)));
    }

    #[test]
    fn left_right_layout_maps_to_a_vertical_cut() {
        // Two panes side by side: 40+ (divider) +39 = 80 wide.
        let l = parse_layout("f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2}").expect("leftright layout");
        assert_eq!(l.pane_ids(), vec![1, 2]);
        match l.to_pane_tree() {
            SplitPane::Split { dir, ratio, a, b } => {
                assert_eq!(dir, SplitDir::V, "{{}} = side by side = a vertical cut");
                assert!(
                    (0.45..0.55).contains(&ratio),
                    "≈even from 40:39, got {ratio}"
                );
                assert_eq!(*a, SplitPane::Leaf(SessionId(1)));
                assert_eq!(*b, SplitPane::Leaf(SessionId(2)));
            }
            SplitPane::Leaf(_) => unreachable!("expected a split, got a leaf"),
        }
    }

    #[test]
    fn top_bottom_layout_maps_to_a_horizontal_cut() {
        // Two panes stacked: 12+ (divider) +11 = 24 tall.
        let l = parse_layout("aaaa,80x24,0,0[80x12,0,0,3,80x11,0,13,4]").expect("topbottom layout");
        match l.to_pane_tree() {
            SplitPane::Split { dir, a, b, .. } => {
                assert_eq!(dir, SplitDir::H, "[] = stacked = a horizontal cut");
                assert_eq!(*a, SplitPane::Leaf(SessionId(3)));
                assert_eq!(*b, SplitPane::Leaf(SessionId(4)));
            }
            SplitPane::Leaf(_) => unreachable!("expected a split, got a leaf"),
        }
    }

    #[test]
    fn nested_and_nary_layout_folds_to_a_binary_tree() {
        // A left column pane, and a right column split top/bottom — three panes.
        let l = parse_layout("bbbb,80x24,0,0{40x24,0,0,1,39x24,41,0[39x12,41,0,2,39x11,41,13,3]}")
            .expect("nested layout");
        assert_eq!(l.pane_ids(), vec![1, 2, 3]);
        // Right side (id 2 over id 3) is itself an H split nested under the V.
        match l.to_pane_tree() {
            SplitPane::Split { dir, a, b, .. } => {
                assert_eq!(dir, SplitDir::V);
                assert_eq!(*a, SplitPane::Leaf(SessionId(1)));
                match *b {
                    SplitPane::Split { dir, a, b, .. } => {
                        assert_eq!(dir, SplitDir::H);
                        assert_eq!(*a, SplitPane::Leaf(SessionId(2)));
                        assert_eq!(*b, SplitPane::Leaf(SessionId(3)));
                    }
                    SplitPane::Leaf(_) => unreachable!("expected nested H split, got a leaf"),
                }
            }
            SplitPane::Leaf(_) => unreachable!("expected outer V split, got a leaf"),
        }
    }

    #[test]
    fn malformed_layout_is_an_error_not_a_panic() {
        assert_eq!(parse_layout(""), Err(LayoutError::Malformed));
        assert!(parse_layout("bd41,not-a-layout").is_err());
    }

    // ── the model reconcile ──────────────────────────────────────────────────

    fn drive(model: &mut TmuxModel, stream: &[u8]) {
        let mut p = Parser::new();
        for note in p.feed(stream) {
            model.apply(note);
        }
    }

    #[test]
    fn model_builds_sessions_windows_and_panes() {
        let mut m = TmuxModel::new();
        drive(
            &mut m,
            b"%session-changed $0 main\n\
              %window-add @0\n\
              %layout-change @0 f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2}\n\
              %window-renamed @0 editor\n",
        );

        assert_eq!(m.current_session(), Some(0));
        assert_eq!(m.session(0).map(TmuxSession::name), Some("main"));
        assert_eq!(m.windows_in_order(), vec![0]);
        assert_eq!(m.window(0).map(TmuxWindow::name), Some("editor"));
        assert_eq!(m.panes_of_window(0), vec![1, 2]);
        // The window maps to a native V split of the two panes.
        assert!(matches!(
            m.window_tree(0),
            Some(SplitPane::Split {
                dir: SplitDir::V,
                ..
            })
        ));
        // Each pane sized to its layout cell (40x24 / 39x24).
        assert_eq!(m.pane(1).map(TmuxPane::size), Some((40, 24)));
        assert_eq!(m.pane(2).map(TmuxPane::size), Some((39, 24)));
    }

    #[test]
    fn layout_change_removes_a_closed_pane() {
        let mut m = TmuxModel::new();
        drive(
            &mut m,
            b"%window-add @0\n\
              %layout-change @0 f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2}\n",
        );
        assert_eq!(m.panes_of_window(0), vec![1, 2]);
        // Pane 2 closed → the window is now a single pane 1.
        drive(&mut m, b"%layout-change @0 bd41,80x24,0,0,1\n");
        assert_eq!(m.panes_of_window(0), vec![1]);
        assert!(m.pane(2).is_none(), "the closed pane was dropped");
        assert!(m.pane(1).is_some(), "the surviving pane stays");
    }

    #[test]
    fn closing_a_window_drops_it_and_its_panes() {
        let mut m = TmuxModel::new();
        drive(
            &mut m,
            b"%window-add @0\n\
              %layout-change @0 f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2}\n\
              %window-close @0\n",
        );
        assert!(m.window(0).is_none());
        assert!(m.windows_in_order().is_empty());
        assert!(m.pane(1).is_none() && m.pane(2).is_none());
    }

    #[test]
    fn exit_is_recorded() {
        let mut m = TmuxModel::new();
        drive(&mut m, b"%exit bye\n");
        assert_eq!(m.exit_reason(), Some("bye"));
    }

    #[test]
    fn session_window_changed_tracks_the_current_window() {
        let mut m = TmuxModel::new();
        drive(
            &mut m,
            b"%session-changed $0 main\n\
              %window-add @0\n\
              %window-add @1\n\
              %session-window-changed $0 @1\n",
        );
        assert_eq!(m.current_window(), Some(1));
        // A foreign session's change is not this client's current window.
        drive(&mut m, b"%session-window-changed $9 @7\n");
        assert_eq!(m.current_window(), Some(1));
        // Closing the current window clears it (the view falls back).
        drive(&mut m, b"%window-close @1\n");
        assert_eq!(m.current_window(), None);
    }

    #[test]
    fn zoom_flags_swap_the_window_tree_to_the_visible_layout() {
        let mut m = TmuxModel::new();
        drive(
            &mut m,
            b"%window-add @0\n\
              %layout-change @0 f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2} f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2} *\n",
        );
        assert!(!m.window(0).expect("window").is_zoomed());
        // Zoom pane 2: the full layout keeps both panes, the visible one is the
        // zoomed pane filling the window, the flags carry `Z`.
        drive(
            &mut m,
            b"%layout-change @0 f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2} bd41,80x24,0,0,2 *Z\n",
        );
        assert!(m.window(0).expect("window").is_zoomed());
        assert_eq!(
            m.window_tree(0),
            Some(SplitPane::Leaf(SessionId(2))),
            "a zoomed window renders its visible (single-pane) arrangement"
        );
        // The hidden pane survives (the full set is the retention truth) and the
        // zoomed pane is sized to the whole window.
        assert!(m.pane(1).is_some(), "the hidden pane must not be dropped");
        assert_eq!(m.pane(2).map(TmuxPane::size), Some((80, 24)));
        // Unzoom: back to the split arrangement + the layout-cell sizes.
        drive(
            &mut m,
            b"%layout-change @0 f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2} f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2} *\n",
        );
        assert!(!m.window(0).expect("window").is_zoomed());
        assert!(matches!(m.window_tree(0), Some(SplitPane::Split { .. })));
        assert_eq!(m.pane(2).map(TmuxPane::size), Some((39, 24)));
    }

    #[test]
    fn window_order_reply_reorders_and_relinks() {
        let mut m = TmuxModel::new();
        drive(
            &mut m,
            b"%session-changed $0 main\n\
              %window-add @0\n\
              %window-add @1\n\
              %window-add @2\n",
        );
        assert_eq!(m.windows_in_order(), vec![0, 1, 2], "id order until told");
        // The order truth: @2 moved to the front, @1 gone from the session,
        // @5 exists but was never streamed (an attach to an existing session).
        m.apply_window_order(&[(2, 0), (0, 1), (5, 2)]);
        assert_eq!(m.windows_in_order(), vec![2, 0, 5]);
        assert!(!m.window(1).expect("window 1").is_linked());
        assert_eq!(m.window(5).and_then(TmuxWindow::session), Some(0));
    }

    #[test]
    fn pane_title_reply_sets_titles() {
        let mut m = TmuxModel::new();
        drive(
            &mut m,
            b"%window-add @0\n\
              %layout-change @0 f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2}\n",
        );
        m.apply_pane_titles(&[(1, "build".to_owned()), (2, "logs".to_owned())]);
        assert_eq!(m.pane(1).map(TmuxPane::title), Some("build"));
        assert_eq!(m.pane(2).map(TmuxPane::title), Some("logs"));
    }

    // ── %output → the widget grid (the pane engine) ──────────────────────────

    #[test]
    fn output_feeds_the_pane_grid() {
        let mut m = TmuxModel::new();
        drive(
            &mut m,
            b"%window-add @0\n\
              %layout-change @0 bd41,80x24,0,0,7\n\
              %output %7 tmux-\\154\\151\\166\\145\n", // "live"
        );
        let term = m.pane_terminal(7).expect("pane 7 engine");
        let screen = lock_unpoisoned(&term).viewport();
        assert!(
            screen.line_text(0).contains("tmux-live"),
            "row 0 = {:?}",
            screen.line_text(0)
        );
    }

    #[test]
    fn output_before_layout_creates_the_pane() {
        // Output can race ahead of the layout; the pane is created on demand.
        let mut m = TmuxModel::new();
        drive(&mut m, b"%output %5 hi\n");
        let term = m.pane_terminal(5).expect("pane 5 created by output");
        assert!(lock_unpoisoned(&term)
            .viewport()
            .line_text(0)
            .contains("hi"));
    }

    // ── the command sink ─────────────────────────────────────────────────────

    #[test]
    fn command_builders_emit_the_exact_tmux_lines() {
        use commands as c;
        assert_eq!(c::select_pane(3), "select-pane -t %3");
        assert_eq!(c::select_window(1), "select-window -t @1");
        assert_eq!(c::split_window(2, SplitDir::V), "split-window -t %2 -h");
        assert_eq!(c::split_window(2, SplitDir::H), "split-window -t %2 -v");
        assert_eq!(c::kill_pane(4), "kill-pane -t %4");
        assert_eq!(c::kill_window(0), "kill-window -t @0");
        assert_eq!(c::new_window(), "new-window");
        assert_eq!(
            c::resize_pane_to(1, 100, 40),
            "resize-pane -t %1 -x 100 -y 40"
        );
        assert_eq!(
            c::resize_pane(1, ResizeDir::Left, 3),
            "resize-pane -t %1 -L 3"
        );
        assert_eq!(c::send_keys(6, b"\x1b[A"), "send-keys -t %6 -H 1b 5b 41");
        assert_eq!(
            c::rename_window(0, "my win"),
            "rename-window -t @0 'my win'"
        );
        // A quote in the name is safely escaped.
        assert_eq!(c::rename_window(0, "a'b"), "rename-window -t @0 'a'\\''b'",);
    }

    #[test]
    fn window_and_pane_op_builders_emit_the_exact_tmux_lines() {
        use commands as c;
        assert_eq!(c::zoom_pane(3), "resize-pane -t %3 -Z");
        assert_eq!(c::break_pane(4), "break-pane -s %4");
        assert_eq!(c::join_pane(4, 2, SplitDir::V), "join-pane -h -s %4 -t @2");
        assert_eq!(c::join_pane(4, 2, SplitDir::H), "join-pane -v -s %4 -t @2");
        assert_eq!(c::swap_panes(1, 5), "swap-pane -d -s %1 -t %5");
        assert_eq!(c::move_window_before(3, 1), "move-window -b -s @3 -t @1");
        assert_eq!(c::move_window_after(1, 3), "move-window -a -s @1 -t @3");
        assert_eq!(c::resize_pane_width(2, 55), "resize-pane -t %2 -x 55");
        assert_eq!(c::resize_pane_height(2, 14), "resize-pane -t %2 -y 14");
        assert_eq!(c::refresh_client_size(120, 40), "refresh-client -C 120x40");
        assert_eq!(
            c::rename_pane(7, "build log"),
            "select-pane -t %7 -T 'build log'"
        );
        // The tagged enumerations stay in step with their parsers.
        assert_eq!(
            c::list_pane_titles(),
            "list-panes -s -F 'pane_title\t#{pane_id}\t#{pane_title}'"
        );
        assert_eq!(
            c::list_window_order(),
            "list-windows -F 'window_order\t#{window_id}\t#{window_index}'"
        );
    }

    #[test]
    fn session_command_builders_emit_the_exact_tmux_lines() {
        use commands as c;
        assert_eq!(c::new_session("dev"), "new-session -s 'dev'");
        assert_eq!(c::attach_session("dev"), "switch-client -t 'dev'");
        assert_eq!(c::detach_client(), "detach-client");
        assert_eq!(c::kill_session("dev"), "kill-session -t 'dev'");
        assert_eq!(
            c::rename_session("dev", "prod"),
            "rename-session -t 'dev' 'prod'"
        );
        // A name with a space stays one tmux argument (single-quote wrapped).
        assert_eq!(c::new_session("my work"), "new-session -s 'my work'");
        // The enumeration format is tab-separated so a spaced name never splits.
        assert_eq!(
            c::list_sessions(),
            "list-sessions -F '#{session_name}\t#{session_attached}\t#{session_windows}'"
        );
    }

    // ── the session enumeration (the picker's source) ────────────────────────

    #[test]
    fn parse_session_list_reads_attached_and_detached() {
        let lines = [
            "main\t1\t3".to_owned(),
            "build\t0\t1".to_owned(),
            "my work\t2\t5".to_owned(), // a spaced name + multi-client attach count
        ];
        let sessions = parse_session_list(&lines);
        assert_eq!(
            sessions,
            vec![
                SessionInfo {
                    name: "main".to_owned(),
                    attached: true,
                    windows: 3,
                },
                SessionInfo {
                    name: "build".to_owned(),
                    attached: false,
                    windows: 1,
                },
                SessionInfo {
                    name: "my work".to_owned(),
                    attached: true,
                    windows: 5,
                },
            ]
        );
    }

    #[test]
    fn parse_session_list_skips_malformed_and_empty_lines() {
        // An empty reply (any non-list command) yields no sessions, so the picker
        // keeps its last good list rather than blanking.
        assert!(parse_session_list(&[]).is_empty());
        // A line with no name, or no attached field, is dropped — never faked.
        let lines = [
            String::new(),
            "\t1\t2".to_owned(),   // empty name
            "orphan".to_owned(),   // no attached field
            "ok\t1\t4".to_owned(), // a good row survives the bad company
        ];
        assert_eq!(
            parse_session_list(&lines),
            vec![SessionInfo {
                name: "ok".to_owned(),
                attached: true,
                windows: 4,
            }]
        );
    }

    // ── the tagged reply parsers (titles + window order) ─────────────────────

    #[test]
    fn parse_pane_titles_reads_tagged_lines_only() {
        let lines = [
            "pane_title\t%1\tvim".to_owned(),
            "pane_title\t%2\ta\ttabbed title".to_owned(), // internal tab kept
            "pane_title\t%3".to_owned(),                  // empty title
            "main\t1\t3".to_owned(),                      // a session-list line
            "pane_title\tnope\tx".to_owned(),             // bad id
        ];
        assert_eq!(
            parse_pane_titles(&lines),
            vec![
                (1, "vim".to_owned()),
                (2, "a\ttabbed title".to_owned()),
                (3, String::new()),
            ]
        );
    }

    #[test]
    fn parse_window_order_reads_tagged_lines_only() {
        let lines = [
            "window_order\t@3\t0".to_owned(),
            "window_order\t@0\t1".to_owned(),
            "main\t1\t3".to_owned(), // a session-list line
            "window_order\t@9".to_owned(),
        ];
        assert_eq!(parse_window_order(&lines), vec![(3, 0), (0, 1)]);
    }

    #[test]
    fn tagged_replies_and_session_lists_never_cross_parse() {
        // The tag column is what routes a reply: each list parses as itself and
        // as nothing else, even for pathological names/titles.
        let titles = ["pane_title\t%5\t42".to_owned()]; // a numeric title
        let order = ["window_order\t@1\t0".to_owned()];
        let sessions = ["pane_title\t1\t2".to_owned()]; // a session named like the tag
        assert!(parse_session_list(&titles).is_empty());
        assert!(parse_session_list(&order).is_empty());
        assert!(parse_pane_titles(&sessions).is_empty());
        assert!(parse_window_order(&sessions).is_empty());
        assert_eq!(parse_session_list(&sessions).len(), 1);
    }

    // ── reply routing through the controller (no live tmux needed) ────────────

    fn offline_controller() -> TmuxController {
        TmuxController {
            channel: None,
            parser: Parser::new(),
            model: TmuxModel::new(),
            status: Status::Connecting,
            all_sessions: Vec::new(),
        }
    }

    #[test]
    fn absorb_routes_each_tagged_reply_to_its_truth() {
        let mut ctrl = offline_controller();
        // Seed a window + two panes.
        let mut p = Parser::new();
        for note in p.feed(
            b"%session-changed $0 main\n\
              %window-add @0\n\
              %layout-change @0 f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2}\n",
        ) {
            let _ = ctrl.absorb(note);
        }
        let reply = |lines: &[&str]| {
            Notification::Reply(CommandReply {
                number: 1,
                flags: 0,
                lines: lines.iter().map(|s| (*s).to_owned()).collect(),
                error: false,
            })
        };
        // A pane-title reply lands on the panes.
        let _ = ctrl.absorb(reply(&["pane_title\t%1\tbuild", "pane_title\t%2\tlogs"]));
        assert_eq!(ctrl.model().pane(1).map(TmuxPane::title), Some("build"));
        // A window-order reply lands on the strip order.
        let _ = ctrl.absorb(reply(&["window_order\t@0\t0"]));
        assert_eq!(ctrl.model().window(0).and_then(TmuxWindow::index), Some(0));
        // A session-list reply lands on the picker cache — not on the model.
        let _ = ctrl.absorb(reply(&["main\t1\t3", "build\t0\t1"]));
        assert_eq!(ctrl.all_sessions().len(), 2);
        // A window-set change asks the caller to re-request the order truth.
        assert!(ctrl.absorb(Notification::WindowAdd { window: 4 }));
        assert!(!ctrl.absorb(Notification::PaneModeChanged { pane: 1 }));
    }

    // ── divider drag → resize-pane (the native path mapped back to cells) ────

    #[test]
    fn resize_for_divider_maps_the_root_boundary() {
        let l = parse_layout("f9d3,80x24,0,0{40x24,0,0,1,39x24,41,0,2}").expect("layout");
        // Drag the vertical divider to a quarter: 79 usable cells → pane 1 gets 20.
        assert_eq!(
            resize_for_divider(&l, NodePath::ROOT, 0.25),
            Some(PaneResize {
                pane: 1,
                dir: LayoutDir::LeftRight,
                cells: 20,
            })
        );
        // The clamp keeps at least one cell each side.
        assert_eq!(
            resize_for_divider(&l, NodePath::ROOT, 0.0).map(|r| r.cells),
            Some(1)
        );
    }

    #[test]
    fn resize_for_divider_descends_the_fold_chain() {
        // A left pane and a right column split top/bottom: the nested divider
        // lives at ROOT.child_b() (the fold's rest-chain), axis TopBottom.
        let l = parse_layout("bbbb,80x24,0,0{40x24,0,0,1,39x24,41,0[39x12,41,0,2,39x11,41,13,3]}")
            .expect("layout");
        assert_eq!(
            resize_for_divider(&l, NodePath::ROOT.child_b(), 0.75),
            Some(PaneResize {
                pane: 2,
                dir: LayoutDir::TopBottom,
                cells: 17, // 0.75 × (12 + 11) ≈ 17
            })
        );
        // A path addressing no divider is honestly nothing.
        assert_eq!(resize_for_divider(&l, NodePath::ROOT.child_a(), 0.5), None);
    }

    #[test]
    fn resize_for_divider_picks_a_full_span_boundary_leaf() {
        // The first side is itself a top/bottom column: either of its panes
        // spans the column's width, and the walk picks the first (pane 1) so a
        // `-x` resize on it moves the outer vertical boundary.
        let l = parse_layout("cccc,80x24,0,0{40x24,0,0[40x12,0,0,1,40x11,0,13,2],39x24,41,0,3}")
            .expect("layout");
        assert_eq!(
            resize_for_divider(&l, NodePath::ROOT, 0.5).map(|r| (r.pane, r.dir)),
            Some((1, LayoutDir::LeftRight))
        );
    }

    // ── the pane-content mount backing (typed input → send-keys) ─────────────

    #[test]
    fn pane_io_routes_input_as_send_keys_and_reads_the_shared_engine() {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let sink = CommandSink::for_tests(tx);
        let engine = Arc::new(Mutex::new(Terminal::with_default_scrollback(20, 4)));
        lock_unpoisoned(&engine).feed(b"ready");

        let io = TmuxPaneIo::new(5, Arc::clone(&engine), sink);
        assert_eq!(io.pane(), 5);
        assert!(!io.is_closed());
        // Typed bytes leave as the exact hex-safe send-keys command line.
        io.send_input(b"hi").expect("send");
        assert_eq!(rx.recv().expect("line"), b"send-keys -t %5 -H 68 69\n");
        // The widget reads the same grid %output feeds.
        assert!(io.with_terminal(|t| t.viewport().line_text(0).contains("ready")));
        // A dead channel refuses input honestly.
        drop(rx);
        assert!(io.send_input(b"x").is_err());
    }

    // ── the controller's honest error state (no live tmux needed) ─────────────

    #[test]
    fn a_missing_binary_yields_an_error_status_not_a_fake_attach() {
        let launch = TmuxLaunch {
            bin: "definitely-not-a-real-tmux-xyzzy".to_owned(),
            ..TmuxLaunch::default()
        };
        let mut ctrl = TmuxController::connect(&launch);
        // Either the spawn is refused at connect (→ Error now), or the exec fails
        // and the dead client closes the channel (→ Error on a later pump). Both
        // reach Error honestly and neither ever fabricates an attach.
        let deadline = Instant::now() + Duration::from_secs(3);
        while !matches!(ctrl.status(), Status::Error(_)) {
            assert_ne!(
                *ctrl.status(),
                Status::Attached,
                "must never fake an attach"
            );
            assert!(
                Instant::now() < deadline,
                "a missing binary never reached an error status (status {:?})",
                ctrl.status()
            );
            ctrl.pump();
            thread::sleep(Duration::from_millis(25));
        }
        assert!(ctrl.model().windows_in_order().is_empty());
    }

    // ── a guarded live attach (runs only where tmux is installed) ─────────────

    fn tmux_available() -> bool {
        Command::new("tmux")
            .arg("-V")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Pump the live controller until `done` holds (10s deadline).
    fn wait_live(
        ctrl: &mut TmuxController,
        what: &str,
        mut done: impl FnMut(&TmuxController) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(ctrl) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            ctrl.pump();
            thread::sleep(Duration::from_millis(25));
        }
    }

    /// The FC-3 **pane** round-trips against a live server: split · zoom on/off ·
    /// rename window · rename pane title · exact-cell resize — each op's command
    /// lands and the `%`-event stream (or tagged reply) reconciles the model,
    /// never a direct mutation.
    fn live_pane_op_round_trips(ctrl: &mut TmuxController, window: u32) -> u32 {
        // Split: one pane becomes two.
        let pane0 = ctrl.model().panes_of_window(window)[0];
        ctrl.send(&commands::split_window(pane0, SplitDir::V))
            .expect("split");
        wait_live(ctrl, "the split's layout-change", |c| {
            c.model().panes_of_window(window).len() == 2
        });

        // Zoom on, then off: the layout-change flags carry the truth.
        ctrl.send(&commands::zoom_pane(pane0)).expect("zoom");
        wait_live(ctrl, "the zoomed layout-change", |c| {
            c.model().window(window).is_some_and(TmuxWindow::is_zoomed)
        });
        ctrl.send(&commands::zoom_pane(pane0)).expect("unzoom");
        wait_live(ctrl, "the unzoomed layout-change", |c| {
            c.model().window(window).is_some_and(|w| !w.is_zoomed())
        });

        // Rename the window: %window-renamed reconciles.
        ctrl.send(&commands::rename_window(window, "fc3"))
            .expect("rename window");
        wait_live(ctrl, "%window-renamed", |c| {
            c.model().window(window).map(TmuxWindow::name) == Some("fc3")
        });

        // Rename a pane title: the tagged list-panes reply reconciles (tmux
        // emits no %-event for titles).
        ctrl.send(&commands::rename_pane(pane0, "alpha"))
            .expect("rename pane");
        ctrl.request_pane_titles().expect("request titles");
        wait_live(ctrl, "the pane-title reply", |c| {
            c.model().pane(pane0).map(TmuxPane::title) == Some("alpha")
        });

        // Resize: the exact-cell command round-trips through layout-change.
        ctrl.send(&commands::resize_pane_width(pane0, 20))
            .expect("resize");
        wait_live(ctrl, "the resize's layout-change", |c| {
            c.model().pane(pane0).map(TmuxPane::size).map(|s| s.0) == Some(20)
        });
        pane0
    }

    /// The FC-3 **window** round-trips: break a pane out (a second window
    /// appears) and reorder it first (the tagged window-order reply reconciles
    /// the strip).
    fn live_window_op_round_trips(ctrl: &mut TmuxController, window: u32, keep: u32) {
        let pane1 = ctrl
            .model()
            .panes_of_window(window)
            .into_iter()
            .find(|p| *p != keep)
            .expect("the split pane");
        ctrl.send(&commands::break_pane(pane1)).expect("break");
        wait_live(ctrl, "the broken-out window", |c| {
            c.model().windows_in_order().len() == 2
        });

        let new_window = ctrl
            .model()
            .windows_in_order()
            .into_iter()
            .find(|w| *w != window)
            .expect("the new window");
        ctrl.send(&commands::move_window_before(new_window, window))
            .expect("move");
        ctrl.request_window_order().expect("request order");
        wait_live(ctrl, "the reordered strip", |c| {
            c.model().windows_in_order().first() == Some(&new_window)
        });
    }

    #[test]
    fn live_control_mode_attaches_when_tmux_is_present() {
        if !tmux_available() {
            eprintln!("skipping: tmux is not installed");
            return;
        }
        // A private socket + session so this never touches the user's tmux and
        // can be torn down cleanly.
        let sock = format!("mde-tmuxfc1-{}", std::process::id());
        let launch = TmuxLaunch {
            args: vec![
                "-L".to_owned(),
                sock.clone(),
                "-CC".to_owned(),
                "new-session".to_owned(),
                "-A".to_owned(),
                "-s".to_owned(),
                "test".to_owned(),
            ],
            ..TmuxLaunch::default()
        };
        let mut ctrl = TmuxController::connect(&launch);

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            ctrl.pump();
            let attached = *ctrl.status() == Status::Attached;
            let has_pane = ctrl
                .model()
                .windows_in_order()
                .first()
                .is_some_and(|w| !ctrl.model().panes_of_window(*w).is_empty());
            if attached && has_pane {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "tmux -CC did not attach + report a pane in time (status {:?})",
                ctrl.status()
            );
            thread::sleep(Duration::from_millis(50));
        }

        // A live window mapped to a real native split tree.
        let window = ctrl.model().windows_in_order()[0];
        assert!(ctrl.model().window_tree(window).is_some());

        // The TMUX-FC-3 round-trips (pane ops, then window ops) — each a GUI
        // command reconciled by the %-event stream / tagged replies.
        let pane0 = live_pane_op_round_trips(&mut ctrl, window);
        live_window_op_round_trips(&mut ctrl, window, pane0);

        // Tear down the private server.
        let _ = ctrl.send("kill-server");
        drop(ctrl);
        let _ = Command::new("tmux")
            .args(["-L", &sock, "kill-server"])
            .output();
    }

    #[test]
    fn live_picker_lists_attached_and_detached_sessions() {
        if !tmux_available() {
            eprintln!("skipping: tmux is not installed");
            return;
        }
        let sock = format!("mde-tmuxfc2-{}", std::process::id());
        let launch = TmuxLaunch {
            args: vec![
                "-L".to_owned(),
                sock.clone(),
                "-CC".to_owned(),
                "new-session".to_owned(),
                "-A".to_owned(),
                "-s".to_owned(),
                "attached".to_owned(),
            ],
            ..TmuxLaunch::default()
        };
        let mut ctrl = TmuxController::connect(&launch);

        // Attach first.
        let deadline = Instant::now() + Duration::from_secs(10);
        while *ctrl.status() != Status::Attached {
            assert!(
                Instant::now() < deadline,
                "tmux -CC never attached (status {:?})",
                ctrl.status()
            );
            ctrl.pump();
            thread::sleep(Duration::from_millis(50));
        }

        // Create a second session WITHOUT switching to it — so the picker must
        // list a genuinely detached session, not just the attached one.
        ctrl.send("new-session -d -s detached")
            .expect("create a detached session");
        // Ask for the full enumeration and pump until both show up.
        ctrl.request_sessions().expect("request the session list");

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            ctrl.pump();
            let sessions = ctrl.all_sessions();
            let attached = sessions.iter().find(|s| s.name == "attached");
            let detached = sessions.iter().find(|s| s.name == "detached");
            if attached.is_some_and(|s| s.attached) && detached.is_some_and(|s| !s.attached) {
                break;
            }
            // The list-sessions reply can race the detached create; re-ask.
            let _ = ctrl.request_sessions();
            assert!(
                Instant::now() < deadline,
                "the picker never listed both sessions (got {:?})",
                ctrl.all_sessions()
            );
            thread::sleep(Duration::from_millis(50));
        }

        // Re-attaching the detached session round-trips a `%session-changed`.
        ctrl.send(&commands::attach_session("detached"))
            .expect("switch to the detached session");

        let _ = ctrl.send("kill-server");
        drop(ctrl);
        let _ = Command::new("tmux")
            .args(["-L", &sock, "kill-server"])
            .output();
    }
}
