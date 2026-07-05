//! TMUX-FC-6 — **mesh tmux**: control-mode over a mesh peer, so a persistent
//! tmux session on any node is driven by the same chrome as a local one.
//!
//! Design: `docs/design/tmux-first-class.md` (#7 mesh — "attach to any node's
//! tmux over the mesh"). The design names the TERM remote/roster SSH-over-overlay
//! seam; in this crate that seam is the **Bus PTY broker** (`crate::remote`,
//! TERM-7/8): the surface never dials SSH itself — the `mackesd` `pty_broker`
//! worker owns the remote-shell lifecycle mesh-side, and the desktop tier leans
//! inward on `mde-bus` only (§6). So this reuses that exact broker rather than
//! opening a second transport.
//!
//! ## The transport
//!
//! [`MeshControlChannel`] opens a broker shell on a peer, `exec`s
//! `tmux -CC new-session -A -s <name>` into it, and pumps the shell's raw output
//! — which, after the `exec`, **is** the tmux control-mode protocol stream —
//! back through the parser, exactly as a local [`crate::tmux::ControlChannel`]
//! pumps its PTY. It presents the identical byte-in/out surface
//! ([`crate::tmux::ControlLink`]), so [`crate::tmux::TmuxController`] + the whole
//! FC-2/3/4/5 chrome control a remote session with the local-session code
//! ([`crate::tmux::TmuxController::over`]).
//!
//! A background worker thread does all Bus I/O (publish the `open` + `exec`, then
//! drain queued command lines to `write` verbs and poll the `state/pty/<id>` log,
//! base64-decoding each output chunk into the control stream), so the surface's
//! per-frame [`crate::tmux::TmuxController::pump`] just reads a queue — the same
//! shape the local channel's PTY threads give it.
//!
//! ## Honesty (§7) + the live gate
//!
//! The whole open→exec→stream→write fold is unit-tested headless against a fake
//! [`crate::remote::PtyBus`] (the shared `remote::test_support::FakeBus`). The
//! **live** leg — a peer actually running the `pty_broker` worker **with tmux
//! installed** — is integration-gated exactly as TERM-8's live remote-shell leg
//! is: a peer with no broker (or no tmux) simply never streams a `%begin`, so the
//! controller stays honestly [`crate::tmux::Status::Connecting`] then closes,
//! never a fabricated attach.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

use crate::remote::{parse_record, verb_close, verb_open, verb_write, PtyBus};
use crate::tmux::{CommandSink, ControlLink};

/// The default poll cadence for the broker state-log drain (a terminal wants low
/// latency; the overlay RTT dominates anyway). Tests pass [`Duration::ZERO`].
const DEFAULT_POLL: Duration = Duration::from_millis(40);

/// Build the shell line that turns a broker login shell into a `tmux -CC` client.
///
/// `exec tmux -CC new-session -A -s <session>` — attach the named session,
/// creating it if absent (the mesh twin of [`crate::tmux::TmuxLaunch::session`]).
/// `exec` replaces the shell so the stream after it is pure control protocol (the
/// pre-`exec` prompt bytes are a handful of `Other` lines the parser drops).
#[must_use]
pub fn attach_command(session: &str) -> String {
    format!("exec tmux -CC new-session -A -s {}", shell_quote(session))
}

/// Single-quote a value for a POSIX shell (`'` → `'\''`), so a session name with
/// a space or a metacharacter reaches `tmux` as one argument.
fn shell_quote(s: &str) -> String {
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

/// Mint a topic-safe unique session id for a peer's control channel — the
/// `state/pty/<id>` key. Distinct prefix from a remote *pane*'s id so the two
/// never collide in the broker's log.
fn mint_id(peer: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let slug: String = peer
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("tmuxcc-{slug}-{}-{n}", std::process::id())
}

/// A `tmux -CC` control channel on a mesh peer, over the Bus PTY broker.
///
/// Duck-types the local [`crate::tmux::ControlChannel`] (raw output out, command
/// lines in, an honest closed flag) via [`ControlLink`], so the controller drives
/// it identically. The worker thread owns all Bus traffic.
pub struct MeshControlChannel {
    /// The command/input queue a [`CommandSink`] pushes to; the worker drains it
    /// to `write` verbs. `Option` so [`Drop`] can close it (→ the worker stops).
    input_tx: Arc<Mutex<Option<Sender<Vec<u8>>>>>,
    /// Decoded control-protocol chunks the worker pushes; `try_recv` reads them.
    output_rx: Receiver<Vec<u8>>,
    /// Set once the peer session ended (`closed`/`unreachable`) or the worker died.
    closed: Arc<AtomicBool>,
    /// Drop signal — the worker publishes `close` and exits when set.
    stop: Arc<AtomicBool>,
    /// The Bus I/O worker.
    worker: Option<JoinHandle<()>>,
    /// The peer short-name (diagnostics).
    peer: String,
}

impl MeshControlChannel {
    /// Dial `tmux -CC` on `peer`, attaching session `session` (creating it if
    /// absent), at an initial grid. Reuses the broker `bus` seam.
    #[must_use]
    pub fn dial(bus: Arc<dyn PtyBus>, peer: &str, session: &str, cols: u16, rows: u16) -> Self {
        Self::dial_with(bus, peer, session, cols, rows, DEFAULT_POLL)
    }

    /// [`Self::dial`] with an explicit poll cadence (the test seam passes
    /// [`Duration::ZERO`] so the fold runs without real sleeps).
    #[must_use]
    pub fn dial_with(
        bus: Arc<dyn PtyBus>,
        peer: &str,
        session: &str,
        cols: u16,
        rows: u16,
        poll: Duration,
    ) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let id = mint_id(peer);
        let (out_tx, output_rx) = mpsc::channel::<Vec<u8>>();
        let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>();
        let input_tx = Arc::new(Mutex::new(Some(in_tx)));
        let closed = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let exec = attach_command(session);

        let worker = thread::Builder::new()
            .name("mde-tmux-mesh".into())
            .spawn({
                // `bus` is moved wholesale into the worker (its only user); the
                // flags are cloned so the channel keeps its own handles.
                let closed = Arc::clone(&closed);
                let stop = Arc::clone(&stop);
                let peer = peer.to_owned();
                move || {
                    pump_broker(
                        &bus, &peer, &id, &exec, cols, rows, &in_rx, &out_tx, &closed, &stop, poll,
                    );
                }
            })
            .ok();

        Self {
            input_tx,
            output_rx,
            closed,
            stop,
            worker,
            peer: peer.to_owned(),
        }
    }

    /// The peer this channel drives (diagnostics).
    #[must_use]
    pub fn peer(&self) -> &str {
        &self.peer
    }
}

impl ControlLink for MeshControlChannel {
    fn try_recv(&self) -> Option<Vec<u8>> {
        self.output_rx.try_recv().ok()
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn send_line(&self, command: &str) -> std::io::Result<()> {
        let mut bytes = command.as_bytes().to_vec();
        bytes.push(b'\n');
        self.input_tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|tx| tx.send(bytes).ok())
            .ok_or_else(|| std::io::ErrorKind::BrokenPipe.into())
    }

    fn sink(&self) -> CommandSink {
        CommandSink::from_input(Arc::clone(&self.input_tx))
    }
}

impl Drop for MeshControlChannel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Drop the input sender so a `send_line` after this fails honestly and
        // the worker's input drain sees a disconnect.
        drop(
            self.input_tx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take(),
        );
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The Bus I/O worker: open the peer shell, `exec` into `tmux -CC`, then bridge
/// the queued command lines (→ `write` verbs) and the peer's output stream
/// (`state/pty/<id>` → base64-decoded control bytes) until the session ends or
/// the channel drops.
#[allow(clippy::too_many_arguments)]
fn pump_broker(
    bus: &Arc<dyn PtyBus>,
    peer: &str,
    id: &str,
    exec: &str,
    cols: u16,
    rows: u16,
    in_rx: &Receiver<Vec<u8>>,
    out_tx: &Sender<Vec<u8>>,
    closed: &AtomicBool,
    stop: &AtomicBool,
    poll: Duration,
) {
    // Open the remote shell; a publish failure (no Bus) is an honest immediate
    // close — never a faked attach.
    if bus.publish(peer, &verb_open(id, cols, rows)).is_err() {
        closed.store(true, Ordering::Release);
        return;
    }
    // Turn that shell into a tmux control client.
    let mut exec_line = exec.as_bytes().to_vec();
    exec_line.push(b'\n');
    let _ = bus.publish(peer, &verb_write(id, &exec_line));

    let mut cursor: Option<String> = None;
    loop {
        // A drop (stop) or a gone input queue → release the control client and
        // exit (the peer's tmux sessions outlive the client, design lock #6).
        if stop.load(Ordering::Acquire) {
            let _ = bus.publish(peer, &verb_close(id));
            break;
        }
        // Drain queued command lines → `write` verbs.
        let mut input_gone = false;
        loop {
            match in_rx.try_recv() {
                Ok(bytes) => {
                    let _ = bus.publish(peer, &verb_write(id, &bytes));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    input_gone = true;
                    break;
                }
            }
        }

        // Read new state records → decode output chunks into the control stream.
        for (ulid, body) in bus.read_since(id, cursor.as_deref()) {
            cursor = Some(ulid);
            let (data, terminal) = fold_record(&body);
            if let Some(bytes) = data {
                if out_tx.send(bytes).is_err() {
                    // The controller dropped its receiver — nothing to feed.
                    closed.store(true, Ordering::Release);
                    let _ = bus.publish(peer, &verb_close(id));
                    return;
                }
            }
            if terminal {
                closed.store(true, Ordering::Release);
                let _ = bus.publish(peer, &verb_close(id));
                return;
            }
        }

        if input_gone {
            let _ = bus.publish(peer, &verb_close(id));
            break;
        }
        thread::sleep(poll);
    }
    closed.store(true, Ordering::Release);
}

/// Fold one `state/pty/<id>` record into `(decoded output, is-terminal)`: a
/// base64 `data` chunk becomes the control-stream bytes; a `closed`/`unreachable`
/// phase ends the session. Pure, so the decode + phase logic is unit-tested
/// without threads or a live Bus. A malformed record is an honest `(None, false)`.
fn fold_record(body: &str) -> (Option<Vec<u8>>, bool) {
    let Some(rec) = parse_record(body) else {
        return (None, false);
    };
    let data = rec.data.as_deref().and_then(|d| B64.decode(d).ok());
    let terminal = matches!(rec.phase.as_str(), "closed" | "unreachable");
    (data, terminal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::test_support::FakeBus;
    use crate::tmux::{Status, TmuxController};

    /// A `state/pty/<id>` record body carrying a base64 output chunk (the shape
    /// `mackesd`'s broker publishes; matches `remote::PtyRecord`).
    fn output_record(seq: u64, bytes: &[u8]) -> String {
        format!(
            "{{\"id\":\"x\",\"phase\":\"open\",\"seq\":{seq},\"data\":\"{}\"}}",
            B64.encode(bytes)
        )
    }

    #[test]
    fn attach_command_execs_control_mode_with_a_quoted_session() {
        assert_eq!(
            attach_command("main"),
            "exec tmux -CC new-session -A -s 'main'"
        );
        // A spaced name reaches tmux as one argument (single-quoted).
        assert_eq!(
            attach_command("my work"),
            "exec tmux -CC new-session -A -s 'my work'"
        );
    }

    #[test]
    fn fold_record_decodes_output_and_flags_the_terminal_phase() {
        // An open+data record → the decoded control bytes, not terminal.
        let (data, terminal) = fold_record(&output_record(1, b"%begin 0 1 0\n"));
        assert_eq!(data.as_deref(), Some(&b"%begin 0 1 0\n"[..]));
        assert!(!terminal);
        // A closed record → terminal, no data.
        let (data, terminal) = fold_record(r#"{"id":"x","phase":"closed","seq":9,"exit":0}"#);
        assert_eq!(data, None);
        assert!(terminal, "a closed phase ends the mesh session");
        // An unreachable peer → terminal (an honest dead-end, never a fake attach).
        let (_, terminal) = fold_record(r#"{"id":"x","phase":"unreachable","seq":0}"#);
        assert!(terminal);
        // Malformed → ignored, never a panic.
        assert_eq!(fold_record("{ not json"), (None, false));
    }

    /// Spin (bounded) until `cond` holds — the idiom for asserting a background
    /// worker's Bus effects without a fixed sleep.
    fn wait_until(mut cond: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !cond() {
            assert!(std::time::Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    const FAST: Duration = Duration::from_millis(1);

    #[test]
    fn dial_opens_the_peer_shell_then_execs_control_mode() {
        let bus = FakeBus::new();
        let ch = MeshControlChannel::dial_with(Arc::new(bus.clone()), "oak", "main", 80, 24, FAST);
        assert_eq!(ch.peer(), "oak");
        let exec_b64 = B64.encode("exec tmux -CC new-session -A -s 'main'\n");
        wait_until(|| {
            let verbs = bus.published_verbs();
            verbs
                .iter()
                .any(|v| v["verb"] == "open" && v["cols"] == 80 && v["rows"] == 24)
                && verbs
                    .iter()
                    .any(|v| v["verb"] == "write" && v["data"] == exec_b64)
        });
        // Every request went to the peer's topic slot.
        assert!(bus.published().iter().all(|p| p.peer == "oak"));
        drop(ch);
    }

    #[test]
    fn a_queued_command_reaches_the_peer_as_a_write_verb() {
        let bus = FakeBus::new();
        let ch = MeshControlChannel::dial_with(Arc::new(bus.clone()), "oak", "main", 80, 24, FAST);
        // Wait for the exec write to settle, then queue a control command.
        wait_until(|| bus.verb_count("write") >= 1);
        ch.send_line("list-sessions").expect("queue");
        let cmd_b64 = B64.encode("list-sessions\n");
        wait_until(|| {
            bus.published_verbs()
                .iter()
                .any(|v| v["verb"] == "write" && v["data"] == cmd_b64)
        });
        drop(ch);
    }

    #[test]
    fn a_dropped_channel_publishes_a_close_so_the_peer_session_is_released() {
        let bus = FakeBus::new();
        let ch = MeshControlChannel::dial_with(Arc::new(bus.clone()), "oak", "main", 80, 24, FAST);
        wait_until(|| bus.verb_count("open") >= 1);
        drop(ch);
        wait_until(|| bus.verb_count("close") >= 1);
    }

    #[test]
    fn a_no_bus_dial_closes_honestly_without_a_fake_attach() {
        // A failing bus → the worker can't even open; the channel reads closed and
        // a controller over it never fabricates an attach (§7).
        let bus = FakeBus::failing("no mesh Bus");
        let ch = MeshControlChannel::dial_with(Arc::new(bus), "oak", "main", 80, 24, FAST);
        wait_until(|| ch.is_closed());
        let mut ctrl = TmuxController::over(Box::new(ch));
        ctrl.pump();
        assert_ne!(
            *ctrl.status(),
            Status::Attached,
            "no fake attach on a dead bus"
        );
    }

    #[test]
    fn a_terminal_phase_from_the_broker_closes_the_channel() {
        // A peer that reports its shell closed → the mesh channel reads closed
        // (the fold's terminal path), exactly as a local %exit would.
        let bus = FakeBus::new();
        let ch = MeshControlChannel::dial_with(Arc::new(bus.clone()), "oak", "main", 80, 24, FAST);
        // The id is internal; the worker reads the log for it. Seed a closed
        // record under the peer's most recent open id by scraping the open verb.
        wait_until(|| bus.verb_count("open") >= 1);
        let id = bus
            .published_verbs()
            .into_iter()
            .find(|v| v["verb"] == "open")
            .and_then(|v| v["id"].as_str().map(str::to_owned))
            .expect("an open verb with an id");
        bus.push_state(&id, r#"{"id":"x","phase":"closed","seq":1,"exit":0}"#);
        wait_until(|| ch.is_closed());
    }

    #[test]
    fn the_control_stream_reconciles_the_model_over_the_transport() {
        // FC-6's core claim: the mesh transport only *moves bytes* — the exact
        // control-mode stream a local PTY delivers reconciles the exact model.
        // Drive a controller over a canned link carrying a real control stream.
        let stream = b"%session-changed $0 mesh\n\
                       %window-add @0\n\
                       %layout-change @0 bd41,80x24,0,0,1\n\
                       %output %1 hi-from-oak\n";
        let mut ctrl = TmuxController::over(Box::new(CannedLink::new(stream)));
        for _ in 0..4 {
            ctrl.pump();
        }
        assert_eq!(*ctrl.status(), Status::Attached);
        assert_eq!(ctrl.model().current_session(), Some(0));
        assert_eq!(ctrl.model().panes_of_window(0), vec![1]);
        let term = ctrl.model().pane_terminal(1).expect("pane engine");
        assert!(term
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .viewport()
            .line_text(0)
            .contains("hi-from-oak"));
    }

    /// A [`ControlLink`] that hands out a fixed byte stream once — the transport
    /// stand-in proving the controller/model reconcile is transport-agnostic.
    struct CannedLink {
        chunks: Mutex<Vec<Vec<u8>>>,
        sink_tx: Arc<Mutex<Option<Sender<Vec<u8>>>>>,
    }

    impl CannedLink {
        fn new(bytes: &[u8]) -> Self {
            Self {
                chunks: Mutex::new(vec![bytes.to_vec()]),
                sink_tx: Arc::new(Mutex::new(Some(mpsc::channel().0))),
            }
        }
    }

    impl ControlLink for CannedLink {
        fn try_recv(&self) -> Option<Vec<u8>> {
            self.chunks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop()
        }
        fn is_closed(&self) -> bool {
            false
        }
        fn send_line(&self, _command: &str) -> std::io::Result<()> {
            Ok(())
        }
        fn sink(&self) -> CommandSink {
            CommandSink::from_input(Arc::clone(&self.sink_tx))
        }
    }
}
