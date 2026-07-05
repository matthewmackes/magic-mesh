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
//! ## Seams left for TMUX-FC-2..8
//! The core is render-agnostic. What it deliberately leaves for the chrome units:
//! - Mounting the per-window [`TmuxModel::window_tree`] + per-pane
//!   [`TmuxModel::pane_terminal`] into a live [`crate::splits::SplitTerminal`] /
//!   [`crate::tabs::TabbedTerminal`] (a `Session::Tmux` widget backing that reads
//!   the shared engine and routes typed input to [`commands::send_keys`]).
//! - The sidebar tree, native Quasar status bar, toolbar + command palette,
//!   context menus (TMUX-FC-2), session create/attach/detach + persistence
//!   (TMUX-FC-3..6), mesh attach (TMUX-FC-7), and the layout presets (TMUX-FC-8).

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
use crate::splits::{clamp_ratio, Pane as SplitPane, SessionId, SplitDir};

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
        /// The visible-layout string (newer tmux), if present.
        visible: Option<String>,
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
    match (win.and_then(|w| parse_id(w, '@')), layout) {
        (Some(window), Some(layout)) => Notification::LayoutChange {
            window,
            layout: layout.to_owned(),
            visible,
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
    active_pane: Option<u32>,
    linked: bool,
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
            Notification::LayoutChange { window, layout, .. } => self.relayout(window, &layout),
            Notification::SessionChanged { session, name } => {
                self.current_session = Some(session);
                self.sessions.entry(session).or_default().name = name;
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
    }

    /// Reconcile a window's pane-set + arrangement from a new layout string:
    /// create panes new to the window (sized to their cells), resize existing
    /// ones, and drop panes no longer present.
    fn relayout(&mut self, window: u32, layout: &str) {
        let Ok(parsed) = parse_layout(layout) else {
            return;
        };
        let ids = parsed.pane_ids();

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

        // Drop panes that used to be in this window but the layout no longer has.
        self.panes
            .retain(|id, p| p.window != Some(window) || ids.contains(id));

        let entry = self.windows.entry(window).or_default();
        entry.layout = Some(parsed);
        if entry.session.is_none() {
            entry.session = self.current_session;
        }
    }

    /// The current attached session id, when a `%session-changed` has arrived.
    #[must_use]
    pub const fn current_session(&self) -> Option<u32> {
        self.current_session
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

    /// The linked windows (those forming the current session's tab strip), by id.
    #[must_use]
    pub fn windows_in_order(&self) -> Vec<u32> {
        self.windows
            .iter()
            .filter(|(_, w)| w.linked)
            .map(|(id, _)| *id)
            .collect()
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
    /// [`crate::splits::SplitTerminal`] tab (leaves keyed by tmux pane id).
    #[must_use]
    pub fn window_tree(&self, window: u32) -> Option<SplitPane> {
        self.windows
            .get(&window)
            .and_then(|w| w.layout.as_ref())
            .map(Layout::to_pane_tree)
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
            },
            Err(err) => Self {
                channel: None,
                parser: Parser::new(),
                model: TmuxModel::new(),
                status: Status::Error(format!("could not start tmux -CC: {err}")),
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
        for chunk in chunks {
            for note in self.parser.feed(&chunk) {
                if let Notification::Exit { reason } = &note {
                    self.status = Status::Exited(reason.clone().unwrap_or_default());
                }
                self.model.apply(note);
            }
        }

        if got && self.status == Status::Connecting {
            self.status = Status::Attached;
        }
        if closed && self.status == Status::Connecting {
            self.status = Status::Error("tmux -CC exited before attaching".to_owned());
        }
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

        // Tear down the private server.
        let _ = ctrl.send("kill-server");
        drop(ctrl);
        let _ = Command::new("tmux")
            .args(["-L", &sock, "kill-server"])
            .output();
    }
}
