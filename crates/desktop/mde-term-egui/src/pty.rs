//! The local PTY layer — a real login shell wired to the VT engine (TERM-2).
//!
//! [`LocalPty`] spawns the user's `$SHELL` (fallback `/bin/sh`) as a **login
//! shell** on a fresh pseudoterminal and pumps it into a TERM-1
//! [`Terminal`]: a reader thread feeds PTY output bytes → the engine, and a
//! writer thread drains queued input bytes → the PTY master, so neither
//! direction ever blocks a caller. Resizes propagate to both the engine grid
//! and the kernel (`TIOCSWINSZ`, so the child sees `SIGWINCH`); closing the
//! session reaps the child — no zombies, no leaked fds.
//!
//! §6 reuse: the PTY plumbing is `alacritty_terminal::tty` — already in this
//! crate's dep graph as the engine's own tty layer. It owns `openpty` (via
//! `rustix-openpty`, the in-lock design choice), the `setsid` + `TIOCSCTTY`
//! session setup, typed `Command` spawn, and a `Drop` that `SIGHUP`s and
//! `wait()`s the child. Re-implementing any of that here (raw `openpty` +
//! hand-rolled session plumbing) would duplicate a mature, battle-tested
//! layer we already compile — and its process-session internals are exactly
//! the code `unsafe_code = "forbid"` keeps out of this crate.
//!
//! §9: the shell is spawned as a **typed argv array** (program + `["-l"]`)
//! through `std::process::Command` — there is no shell-interpolated command
//! string anywhere on this path.

use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::{env, io, thread};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty;

use crate::engine::{Terminal, DEFAULT_SCROLLBACK};

/// The shell used when neither an explicit program nor `$SHELL` is available.
const FALLBACK_SHELL: &str = "/bin/sh";

/// Read chunk size for the PTY→engine pump. One kernel PTY buffer is ~8 KiB,
/// so this drains a full buffer per `read` under heavy output.
const READ_CHUNK: usize = 8192;

/// How a [`LocalPty`] session is spawned.
///
/// The defaults are the design lock (Q10): the user's `$SHELL` as a login
/// shell, inheriting the platform cwd + env, on an 80×24 grid with the
/// engine's default scrollback soft-cap.
#[derive(Clone, Debug)]
pub struct SpawnOptions {
    /// Visible grid columns (clamped to at least 1).
    pub cols: u16,
    /// Visible grid rows (clamped to at least 1).
    pub rows: u16,
    /// Scrollback soft-cap in lines (see [`DEFAULT_SCROLLBACK`]).
    pub scrollback: usize,
    /// Explicit shell program. `None` resolves `$SHELL`, then
    /// [`FALLBACK_SHELL`]. Always spawned as a typed argv array (§9).
    pub shell: Option<String>,
    /// Startup directory. `None` inherits the calling process's cwd.
    pub cwd: Option<PathBuf>,
    /// Extra environment for the child, **on top of** the inherited process
    /// env (`std::process::Command` inherits by default; these override).
    pub env: Vec<(String, String)>,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            scrollback: DEFAULT_SCROLLBACK,
            shell: None,
            cwd: None,
            env: Vec::new(),
        }
    }
}

/// Resolve the shell program: explicit override, else `$SHELL`, else
/// [`FALLBACK_SHELL`]. Pure so the precedence is unit-testable; empty strings
/// are treated as unset (an empty `$SHELL` must not become the program).
fn resolve_shell(explicit: Option<String>, env_shell: Option<String>) -> String {
    explicit
        .filter(|s| !s.is_empty())
        .or_else(|| env_shell.filter(|s| !s.is_empty()))
        .unwrap_or_else(|| FALLBACK_SHELL.to_owned())
}

/// The login-shell argv for `program`: `[program, "-l"]` as a **typed array**.
///
/// `-l` is the login flag every common shell accepts (bash/zsh/fish/dash —
/// `--login` is not universal, and `alacritty_terminal::tty` offers no argv[0]
/// control for the `-shell` convention). Pure, so §9 "argv array, never a
/// shell string" is asserted by a unit test rather than trusted.
fn login_shell_argv(program: String) -> (String, Vec<String>) {
    (program, vec!["-l".to_owned()])
}

/// Lock a mutex, riding through poisoning: a panicked pump thread must not
/// wedge the surface (the terminal state stays readable; the session is
/// already dying).
fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A live local shell session: child process + PTY + the engine pumps.
///
/// Dropping the session closes it cleanly: the input queue closes, the child
/// is `SIGHUP`ed and reaped (`alacritty_terminal::tty::Pty::drop`), the PTY
/// fds close, and both pump threads are joined.
pub struct LocalPty {
    terminal: Arc<Mutex<Terminal>>,
    /// `Option` so the PTY (whose `Drop` reaps the child) can be released
    /// early by whichever side notices the end first: the reader pump on
    /// child exit, or [`Drop`] on session close.
    pty: Arc<Mutex<Option<tty::Pty>>>,
    /// Shared with the reader pump, which closes the queue on child exit —
    /// a PTY master buffers writes even with no slave left (Linux), so the
    /// writer pump alone would never observe the death and the input path
    /// would stay "open" forever.
    input_tx: Arc<Mutex<Option<Sender<Vec<u8>>>>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    output_closed: Arc<AtomicBool>,
    child_pid: u32,
}

impl LocalPty {
    /// Spawn the shell on a fresh PTY and start the engine pumps.
    ///
    /// # Errors
    ///
    /// Anything the OS refuses: `openpty` failure, an unresolvable user, a
    /// missing shell binary, or fd duplication for the pump threads.
    pub fn spawn(opts: SpawnOptions) -> io::Result<Self> {
        let cols = opts.cols.max(1);
        let rows = opts.rows.max(1);

        let (program, args) = login_shell_argv(resolve_shell(opts.shell, env::var("SHELL").ok()));

        // The child's terminal identity, set per-session (never via the
        // process-global `tty::setup_env`, which would mutate the whole
        // surface's env): the engine is alacritty's, whose escape support is
        // xterm-256color-compatible, and truecolor is a design lock (Q20).
        // `SHELL` mirrors the resolved program, as a login(1) would set it.
        let mut child_env = vec![
            ("TERM".to_owned(), "xterm-256color".to_owned()),
            ("COLORTERM".to_owned(), "truecolor".to_owned()),
            ("SHELL".to_owned(), program.clone()),
        ];
        child_env.extend(opts.env);

        let config = tty::Options {
            shell: Some(tty::Shell::new(program, args)),
            working_directory: opts.cwd,
            hold: false,
            env: child_env.into_iter().collect(),
        };
        let window_size = WindowSize {
            num_lines: rows,
            num_cols: cols,
            // Pixel metrics are unknown at this layer (the egui pane, TERM-3,
            // owns glyph geometry); 0 is the "unspecified" winsize convention.
            cell_width: 0,
            cell_height: 0,
        };

        // `tty::new` opens the PTY pair, spawns the argv (typed `Command` —
        // §9), makes the slave the child's stdio + controlling terminal, and
        // hands back the master. `window_id` only feeds cosmetic env vars
        // (`WINDOWID`); 0 = "no X11 window".
        let pty = tty::new(&config, window_size, 0)?;
        let child_pid = pty.child().id();

        // `tty::new` force-sets the master non-blocking (alacritty's own event
        // loop polls it). Our pumps are dedicated blocking threads, so flip it
        // back: O_NONBLOCK lives on the shared file description, one fcntl
        // covers both dup'd handles below.
        let flags = rustix::fs::fcntl_getfl(pty.file())?;
        rustix::fs::fcntl_setfl(pty.file(), flags - rustix::fs::OFlags::NONBLOCK)?;

        let reader_file = pty.file().try_clone()?;
        let writer_file = pty.file().try_clone()?;

        let terminal = Arc::new(Mutex::new(Terminal::new(
            usize::from(cols),
            usize::from(rows),
            opts.scrollback,
        )));
        let pty = Arc::new(Mutex::new(Some(pty)));
        let output_closed = Arc::new(AtomicBool::new(false));
        let (tx, input_rx) = mpsc::channel::<Vec<u8>>();
        let input_tx = Arc::new(Mutex::new(Some(tx)));

        let reader = thread::Builder::new()
            .name("mde-term-pty-read".into())
            .spawn({
                let terminal = Arc::clone(&terminal);
                let pty = Arc::clone(&pty);
                let input_tx = Arc::clone(&input_tx);
                let output_closed = Arc::clone(&output_closed);
                move || {
                    pump_output(reader_file, &terminal, &output_closed);
                    // The master hit EOF/EIO — the child is gone (its slave fds
                    // closed). Release the PTY now so the child is reaped
                    // promptly, not only at session close (`Drop` finding `None`
                    // simply skips), and close the input queue so the writer
                    // pump exits and `send_input` reports the death honestly.
                    drop(lock_unpoisoned(&pty).take());
                    drop(lock_unpoisoned(&input_tx).take());
                }
            })?;

        let writer = thread::Builder::new()
            .name("mde-term-pty-write".into())
            .spawn(move || pump_input(writer_file, &input_rx))?;

        Ok(Self {
            terminal,
            pty,
            input_tx,
            reader: Some(reader),
            writer: Some(writer),
            output_closed,
            child_pid,
        })
    }

    /// The shared engine state. The reader pump feeds it; the surface (and
    /// tests) snapshot it via [`Terminal::viewport`]/[`Terminal::full`].
    #[must_use]
    pub fn terminal(&self) -> Arc<Mutex<Terminal>> {
        Arc::clone(&self.terminal)
    }

    /// Run `f` against the current engine state (a convenience over
    /// [`Self::terminal`] that scopes the lock).
    pub fn with_terminal<R>(&self, f: impl FnOnce(&Terminal) -> R) -> R {
        f(&lock_unpoisoned(&self.terminal))
    }

    /// Queue `bytes` for the child's input. Never blocks: the writer pump
    /// performs the actual PTY write.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::BrokenPipe`] once the session's write side is gone (the
    /// child exited or the session is closing).
    pub fn send_input(&self, bytes: &[u8]) -> io::Result<()> {
        lock_unpoisoned(&self.input_tx)
            .as_ref()
            .and_then(|tx| tx.send(bytes.to_vec()).ok())
            .ok_or_else(|| ErrorKind::BrokenPipe.into())
    }

    /// Resize the session to `cols × rows`: the engine grid reflows, and the
    /// kernel winsize updates (`TIOCSWINSZ`), which delivers `SIGWINCH` to the
    /// child's foreground process group. A no-op on the PTY side after the
    /// child has exited.
    pub fn resize(&self, cols: u16, rows: u16) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        lock_unpoisoned(&self.terminal).resize(usize::from(cols), usize::from(rows));
        if let Some(pty) = lock_unpoisoned(&self.pty).as_mut() {
            pty.on_resize(WindowSize {
                num_lines: rows,
                num_cols: cols,
                cell_width: 0,
                cell_height: 0,
            });
        }
    }

    /// `true` once the PTY output stream has ended — the child exited (or the
    /// master otherwise closed) and no further engine updates will arrive.
    #[must_use]
    pub fn is_output_closed(&self) -> bool {
        self.output_closed.load(Ordering::Acquire)
    }

    /// The shell's process id (for diagnostics and the reap tests).
    #[must_use]
    pub const fn child_pid(&self) -> u32 {
        self.child_pid
    }
}

impl Drop for LocalPty {
    fn drop(&mut self) {
        // 1. Close the input queue: the writer pump drains and exits.
        drop(lock_unpoisoned(&self.input_tx).take());
        // 2. Release the PTY (unless the reader already did on child exit):
        //    its Drop SIGHUPs the child and wait()s — the reap. Take it out
        //    of the lock *before* dropping so the wait doesn't hold the lock.
        let pty = lock_unpoisoned(&self.pty).take();
        drop(pty);
        // 3. Both pumps now unblock (the dead child closes the PTY slave, so
        //    reads return EOF/EIO and writes EPIPE/EIO) — join them so no
        //    thread outlives the session.
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

/// The PTY→engine pump: blocking-read the master, feed the engine, until the
/// stream ends. EIO is the normal Linux "child exited, no slave left" ending,
/// EOF the BSD-style one; both close the stream. Marks `output_closed` last.
fn pump_output(mut file: std::fs::File, terminal: &Mutex<Terminal>, output_closed: &AtomicBool) {
    let mut buf = [0_u8; READ_CHUNK];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => lock_unpoisoned(terminal).feed(&buf[..n]),
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    output_closed.store(true, Ordering::Release);
}

/// The input→PTY pump: drain queued chunks into the master. Ends when the
/// queue closes (session drop) or the PTY write side dies (child exited).
fn pump_input(mut file: std::fs::File, input_rx: &Receiver<Vec<u8>>) {
    while let Ok(chunk) = input_rx.recv() {
        if file.write_all(&chunk).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// Poll `probe` until it returns `Some` or the deadline passes. The PTY
    /// pumps are asynchronous, so tests wait on observed state — never a bare
    /// sleep.
    fn wait_for<R>(what: &str, mut probe: impl FnMut() -> Option<R>) -> R {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(r) = probe() {
                return r;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(25));
        }
    }

    /// True while `pid` exists (running **or zombie** — a zombie keeps its
    /// `/proc` entry, so this going false proves the child was reaped).
    fn pid_exists(pid: u32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }

    fn spawn_sh() -> LocalPty {
        LocalPty::spawn(SpawnOptions {
            shell: Some("/bin/sh".to_owned()),
            ..SpawnOptions::default()
        })
        .expect("spawn /bin/sh on a fresh PTY")
    }

    /// The full-snapshot text (scrollback + viewport) as one string.
    fn full_text(session: &LocalPty) -> String {
        session.with_terminal(|term| {
            let full = term.full();
            (0..full.rows())
                .map(|row| full.line_text(row))
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    fn wait_for_text(session: &LocalPty, needle: &str) {
        wait_for(needle, || full_text(session).contains(needle).then_some(()));
    }

    // --- pure resolution (the §9 argv shape + shell precedence) ---

    #[test]
    fn shell_resolution_prefers_explicit_then_env_then_fallback() {
        let explicit = Some("/bin/zsh".to_owned());
        let env = Some("/bin/bash".to_owned());
        assert_eq!(resolve_shell(explicit.clone(), env.clone()), "/bin/zsh");
        assert_eq!(resolve_shell(None, env), "/bin/bash");
        assert_eq!(resolve_shell(None, None), FALLBACK_SHELL);
        // Empty strings are "unset", never a spawnable program.
        assert_eq!(
            resolve_shell(Some(String::new()), Some(String::new())),
            FALLBACK_SHELL
        );
        assert_eq!(resolve_shell(Some(String::new()), explicit), "/bin/zsh");
    }

    #[test]
    fn login_argv_is_a_typed_array_with_the_login_flag() {
        let (program, args) = login_shell_argv("/bin/bash".to_owned());
        assert_eq!(program, "/bin/bash");
        // §9: exactly the program + the login flag — no `-c`, no command
        // string for a shell to interpret.
        assert_eq!(args, vec!["-l".to_owned()]);
    }

    // --- runtime smoke on a real PTY (the farm allows process spawn) ---

    #[test]
    fn shell_runs_and_output_reaches_the_engine() {
        let session = spawn_sh();
        // Quote the tail so the *echoed input* line can't contain the needle:
        // only the command's real output matches "hello-term".
        session
            .send_input(b"echo hello-'term'\n")
            .expect("queue input");
        wait_for_text(&session, "hello-term");
    }

    #[test]
    fn ls_output_reaches_the_engine() {
        // Runtime-smoke per the acceptance: `ls` of a directory with a known
        // entry shows that entry. `/` always has `usr` on the farm's Fedora.
        let session = spawn_sh();
        session.send_input(b"ls /\n").expect("queue input");
        wait_for_text(&session, "usr");
    }

    #[test]
    fn spawn_inherits_cwd_and_honours_an_explicit_one() {
        let session = LocalPty::spawn(SpawnOptions {
            shell: Some("/bin/sh".to_owned()),
            cwd: Some(PathBuf::from("/tmp")),
            ..SpawnOptions::default()
        })
        .expect("spawn with explicit cwd");
        session.send_input(b"pwd\n").expect("queue input");
        wait_for_text(&session, "/tmp");

        // No explicit cwd → the child inherits this process's cwd.
        let here = env::current_dir().expect("test process cwd");
        let session = spawn_sh();
        session.send_input(b"pwd\n").expect("queue input");
        wait_for_text(&session, &here.display().to_string());
    }

    #[test]
    fn child_env_layers_over_the_inherited_process_env() {
        let session = LocalPty::spawn(SpawnOptions {
            shell: Some("/bin/sh".to_owned()),
            env: vec![("MDE_TERM_SMOKE".to_owned(), "pty-mark".to_owned())],
            ..SpawnOptions::default()
        })
        .expect("spawn with extra env");
        // The extra var is present, and an inherited var ($HOME) matches the
        // parent's — quoted tails keep the echoed input from matching.
        session
            .send_input(b"echo got=$MDE_TERM_SMOKE'' term=$TERM''\n")
            .expect("queue input");
        wait_for_text(&session, "got=pty-mark");
        wait_for_text(&session, "term=xterm-256color");

        let home = env::var("HOME").expect("test process HOME");
        session
            .send_input(b"echo home=$HOME''\n")
            .expect("queue input");
        wait_for_text(&session, &format!("home={home}"));
    }

    #[test]
    fn shell_is_a_login_shell() {
        // bash reports its own login flag honestly; the farm's Fedora always
        // has /bin/bash. This observes the *runtime* effect of `-l`, not just
        // the argv shape.
        let session = LocalPty::spawn(SpawnOptions {
            shell: Some("/bin/bash".to_owned()),
            ..SpawnOptions::default()
        })
        .expect("spawn /bin/bash");
        session
            .send_input(b"shopt -q login_shell && echo is-'login'\n")
            .expect("queue input");
        wait_for_text(&session, "is-login");
    }

    #[test]
    fn resize_updates_the_kernel_winsize_and_the_engine_grid() {
        let session = spawn_sh();
        session.resize(100, 40);
        // The child reads the new winsize via ioctl — `stty size` prints
        // "rows cols". The resize ioctl is synchronous, so it is already
        // visible when the shell runs the queued command.
        session.send_input(b"stty size\n").expect("queue input");
        wait_for_text(&session, "40 100");
        session.with_terminal(|term| {
            assert_eq!((term.cols(), term.rows()), (100, 40), "engine grid follows");
        });
    }

    #[test]
    fn child_exit_closes_output_and_reaps_without_a_session_close() {
        let session = spawn_sh();
        let pid = session.child_pid();
        assert!(pid_exists(pid), "shell is alive after spawn");
        session.send_input(b"exit\n").expect("queue input");
        wait_for("output stream to close", || {
            session.is_output_closed().then_some(())
        });
        // The reader pump releases the PTY on stream end → the child is
        // reaped promptly (a zombie would still hold /proc/<pid>).
        wait_for("child reap after exit", || (!pid_exists(pid)).then_some(()));
        // The dead session refuses input honestly.
        wait_for("input path to close", || {
            session.send_input(b"x").is_err().then_some(())
        });
    }

    #[test]
    fn dropping_the_session_reaps_a_live_child() {
        let session = spawn_sh();
        let pid = session.child_pid();
        // Prove the shell is genuinely up (prompt echo round-trip), then
        // close the session out from under it.
        session.send_input(b"echo up-'now'\n").expect("queue input");
        wait_for_text(&session, "up-now");
        assert!(pid_exists(pid), "shell is alive before drop");
        drop(session);
        // Drop is synchronous: SIGHUP + wait() completed (directly or via the
        // joined reader pump), so the pid is gone — not a zombie — already.
        assert!(!pid_exists(pid), "child reaped by session drop");
    }
}
