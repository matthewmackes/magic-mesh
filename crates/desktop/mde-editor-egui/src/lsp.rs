//! EDITOR-LSP-1 — the **LSP client subsystem**: language-server lifecycle,
//! stdio JSON-RPC transport, document sync, and a typed diagnostics store.
//!
//! The editor talks to real language servers (rust-analyzer, pylsp, …) as
//! child processes over stdio, speaking LSP's JSON-RPC framing
//! (`Content-Length` headers). The subsystem is deliberately a *client only*:
//! the framing is hand-rolled below (~40 lines) rather than pulling a whole
//! tower-lsp-style server stack; the protocol *types* come from the pure-Rust
//! `lsp-types` crate through `serde_json`.
//!
//! What lives here:
//!
//! * **Registry + probe** — [`server_spec`] maps a [`Language`] to its
//!   well-known server command; [`find_in_path`] probes whether that binary
//!   exists on this host. A missing binary is surfaced as the typed, honest
//!   [`LspState::Unavailable`] (§7 — a gated state, never a faked session);
//!   a language with no registered server is [`LspState::NoServer`].
//! * **Lifecycle** — [`LspClient::start`] spawns the server, performs the
//!   `initialize`/`initialized` handshake on the reader thread, and
//!   [`LspClient::shutdown`] runs `shutdown`/`exit`. States are observable
//!   through [`LspClient::state`].
//! * **Document sync** — the seam the editor panel calls (EDITOR-LSP-2 wires
//!   it): [`LspClient::on_open`] / [`LspClient::on_change`] /
//!   [`LspClient::on_close`] send `didOpen`/`didChange`/`didClose` with
//!   full-text sync and a per-document version counter. Notifications issued
//!   while the handshake is still in flight are queued and flushed the moment
//!   the server reports ready (LSP forbids traffic before `initialized`).
//! * **Inbound** — a reader thread parses server frames;
//!   `textDocument/publishDiagnostics` is folded into a typed
//!   path → [`Diagnostic`] store the UI reads via
//!   [`LspClient::diagnostics_for`] (bump-counter:
//!   [`LspClient::diagnostics_epoch`]). Other notifications are ignored
//!   gracefully, and server→client *requests* get an honest
//!   `MethodNotFound` reply so no server ever stalls awaiting us.
//!
//! Threading matches the crate's std-first style (the `mde-term-egui` PTY
//! pump idiom): two named threads (reader + writer) and an `mpsc` channel —
//! no async runtime. Every public call is non-blocking; the UI thread never
//! waits on the server.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::highlight::Language;

// ---------------------------------------------------------------------------
// Diagnostics — the typed store the UI consumes (EDITOR-LSP-2's gutter).
// ---------------------------------------------------------------------------

/// Diagnostic severity, ordered so that a *worse* severity compares greater
/// (`Hint < Information < Warning < Error`) — a gutter can take the `max()`
/// of a line's diagnostics to pick its marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// A hint (e.g. an unused-variable underline hint).
    Hint,
    /// Informational.
    Information,
    /// A warning.
    Warning,
    /// An error. Also the fallback when a server omits the severity — the
    /// LSP spec tells clients to treat an absent severity as an error.
    Error,
}

impl Severity {
    /// Fold `lsp-types`' optional severity into the typed local one.
    fn from_lsp(severity: Option<lsp_types::DiagnosticSeverity>) -> Self {
        match severity {
            Some(s) if s == lsp_types::DiagnosticSeverity::WARNING => Self::Warning,
            Some(s) if s == lsp_types::DiagnosticSeverity::INFORMATION => Self::Information,
            Some(s) if s == lsp_types::DiagnosticSeverity::HINT => Self::Hint,
            // Absent, ERROR, or an out-of-spec value: honest worst case.
            _ => Self::Error,
        }
    }
}

/// One folded diagnostic for a document, as published by the server.
///
/// Positions are the LSP wire values: **zero-based** line / character, where
/// `character` counts UTF-16 code units (the protocol default). EDITOR-LSP-2
/// converts to buffer char offsets when it paints the gutter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Severity (worst-case [`Severity::Error`] when the server omits it).
    pub severity: Severity,
    /// Zero-based start line.
    pub start_line: u32,
    /// Zero-based start column (UTF-16 code units).
    pub start_character: u32,
    /// Zero-based end line.
    pub end_line: u32,
    /// Zero-based end column (UTF-16 code units), exclusive.
    pub end_character: u32,
    /// The human-readable message.
    pub message: String,
    /// The producing tool, when reported (e.g. `rustc`, `clippy`).
    pub source: Option<String>,
}

impl Diagnostic {
    /// Fold an `lsp-types` diagnostic into the flat local shape.
    fn from_lsp(d: lsp_types::Diagnostic) -> Self {
        Self {
            severity: Severity::from_lsp(d.severity),
            start_line: d.range.start.line,
            start_character: d.range.start.character,
            end_line: d.range.end.line,
            end_character: d.range.end.character,
            message: d.message,
            source: d.source,
        }
    }
}

// ---------------------------------------------------------------------------
// Navigation results — the typed shapes the UI-driven requests
// (definition / references / rename / formatting, EDITOR-LSP-3) fold their
// responses into. Positions are the LSP wire values — **zero-based** line +
// UTF-16 `character` — exactly like [`Diagnostic`]; the `lsp_nav` UI resolves
// them onto rope char offsets when it jumps or applies an edit.
// ---------------------------------------------------------------------------

/// A zero-based LSP range (line + UTF-16 `character`): the wire span shared by
/// locations, text edits, and rename spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LspRange {
    /// Zero-based start line.
    pub start_line: u32,
    /// Zero-based start column (UTF-16 code units).
    pub start_character: u32,
    /// Zero-based end line.
    pub end_line: u32,
    /// Zero-based end column (UTF-16 code units), exclusive.
    pub end_character: u32,
}

/// A resolved location: a file plus the [`LspRange`] within it — a
/// goto-definition target or one find-references hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// The file the location points into.
    pub path: PathBuf,
    /// The zero-based range within the file.
    pub range: LspRange,
}

/// One text edit: replace [`range`](Self::range) with
/// [`new_text`](Self::new_text) — a formatting edit or one entry of a rename
/// [`WorkspaceEdit`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    /// The zero-based range the edit replaces.
    pub range: LspRange,
    /// The replacement text.
    pub new_text: String,
}

/// A workspace edit: the per-file text edits a rename returns, in the server's
/// order. A cross-file edit is honestly represented — every affected file with
/// its own edit list (§7).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceEdit {
    /// The affected files, each with its ordered list of edits.
    pub changes: Vec<(PathBuf, Vec<TextEdit>)>,
}

impl WorkspaceEdit {
    /// Whether the edit touches nothing (an honest "nothing to rename").
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// A `prepareRename` result.
///
/// The span of the symbol under the cursor plus the server's suggested
/// placeholder (the rename box's initial text) when it gave one. A `None` reply
/// from the server means "not renameable here"; the UI then falls back to the
/// word under the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrepareRename {
    /// The span of the renameable symbol.
    pub range: LspRange,
    /// The server's suggested placeholder, when it provided one.
    pub placeholder: Option<String>,
}

/// A completed reply to one UI-driven navigation request (EDITOR-LSP-3), folded
/// to the flat local types and delivered to the panel through the
/// [`LspClient::take_replies`] poll seam.
///
/// Honest by construction (§7): a server that returns `null` yields an empty
/// result (no locations, no edits) — never a fabricated one — and a gated /
/// absent server never enqueues a request at all, so no reply is ever faked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LspReply {
    /// `textDocument/definition` — the target locations (the UI jumps to the
    /// first; empty means "no definition found").
    Definition(Vec<Location>),
    /// `textDocument/references` — every reference (the UI lists them).
    References(Vec<Location>),
    /// `textDocument/prepareRename` — the renameable span + placeholder, or
    /// `None` when the position is not renameable.
    PrepareRename(Option<PrepareRename>),
    /// `textDocument/rename` — the cross-file workspace edit to apply.
    Rename(WorkspaceEdit),
    /// `textDocument/formatting` — the edits to apply to the buffer.
    Format(Vec<TextEdit>),
}

// ---------------------------------------------------------------------------
// State — every phase of a session is a typed, honest state (§7).
// ---------------------------------------------------------------------------

/// The observable state of an [`LspClient`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LspState {
    /// No language server is registered for this language at all
    /// (e.g. Markdown — a prose surface; see [`server_spec`]).
    NoServer {
        /// The language that has no registered server.
        language: Language,
    },
    /// A server is registered but its binary is **absent on this host** —
    /// the honest gated state (§7): no session is faked, and the UI can tell
    /// the operator exactly which command to install.
    Unavailable {
        /// The language whose server is missing.
        language: Language,
        /// The command that was probed for and not found (e.g.
        /// `rust-analyzer`).
        cmd: String,
    },
    /// The server process is spawned and the `initialize` handshake is in
    /// flight. Document-sync notifications issued now are queued and flushed
    /// on [`LspState::Running`].
    Initializing,
    /// The handshake completed; the session is live.
    Running,
    /// The session died: the spawn failed, `initialize` was rejected, or the
    /// server exited/closed its stream without a clean shutdown.
    Failed {
        /// What went wrong, for the status surface.
        reason: String,
    },
    /// Cleanly shut down via [`LspClient::shutdown`].
    Stopped,
}

// ---------------------------------------------------------------------------
// Registry + probe — which server serves a language, and is it installed?
// ---------------------------------------------------------------------------

/// A language server's launch recipe: the executable plus the arguments that
/// put it in stdio mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerSpec {
    /// The executable name looked up on `PATH`.
    pub program: &'static str,
    /// Arguments that select the stdio transport.
    pub args: &'static [&'static str],
}

/// The well-known language server for `language`, or `None` when none is
/// registered (Markdown: a prose surface — diagnostics would be noise).
///
/// The set mirrors the grammar languages the editor already highlights; each
/// entry is the ecosystem-standard server in its stdio mode.
#[must_use]
pub const fn server_spec(language: Language) -> Option<ServerSpec> {
    match language {
        Language::Rust => Some(ServerSpec {
            program: "rust-analyzer",
            args: &[],
        }),
        Language::Python => Some(ServerSpec {
            program: "pylsp",
            args: &[],
        }),
        Language::JavaScript | Language::TypeScript => Some(ServerSpec {
            program: "typescript-language-server",
            args: &["--stdio"],
        }),
        Language::Json => Some(ServerSpec {
            program: "vscode-json-language-server",
            args: &["--stdio"],
        }),
        Language::Toml => Some(ServerSpec {
            program: "taplo",
            args: &["lsp", "stdio"],
        }),
        Language::Bash => Some(ServerSpec {
            program: "bash-language-server",
            args: &["start"],
        }),
        Language::Markdown => None,
    }
}

impl Language {
    /// The LSP `languageId` string sent in `textDocument/didOpen`.
    #[must_use]
    pub const fn lsp_id(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Json => "json",
            Self::Toml => "toml",
            Self::Markdown => "markdown",
            Self::Bash => "shellscript",
        }
    }
}

/// `which`-style probe: the first directory on `PATH` holding an executable
/// regular file named `program`, or `None` — the [`LspState::Unavailable`]
/// gate.
#[must_use]
pub fn find_in_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    find_in_dirs(program, std::env::split_paths(&path))
}

/// The probe over an explicit directory list (unit-testable without mutating
/// the process environment).
fn find_in_dirs(program: &str, dirs: impl Iterator<Item = PathBuf>) -> Option<PathBuf> {
    for dir in dirs {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(program);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// A regular file with any execute bit set (the platform is Linux-only —
/// the shell is DRM-native).
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

// ---------------------------------------------------------------------------
// file:// URIs — LSP identifies documents by URI; the store keys by PathBuf.
// ---------------------------------------------------------------------------

/// RFC 3986 unreserved bytes, plus `/` kept literal inside paths.
const fn is_unreserved_path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/')
}

/// Percent-encode an absolute path as a `file://` URI. `None` for relative
/// or non-UTF-8 paths (LSP documents are absolute; the fleet's paths are
/// UTF-8).
fn path_to_file_uri(path: &Path) -> Option<String> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    if !path.is_absolute() {
        return None;
    }
    let text = path.to_str()?;
    let mut uri = String::with_capacity(text.len() + 8);
    uri.push_str("file://");
    for &b in text.as_bytes() {
        if is_unreserved_path_byte(b) {
            uri.push(char::from(b));
        } else {
            uri.push('%');
            uri.push(char::from(HEX[usize::from(b >> 4)]));
            uri.push(char::from(HEX[usize::from(b & 0x0F)]));
        }
    }
    Some(uri)
}

/// Decode a `file://` URI back to a path — the inverse of
/// [`path_to_file_uri`], tolerant of an authority component
/// (`file://localhost/x`). `None` for non-file schemes, malformed percent
/// escapes, or non-UTF-8 results.
fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path_part = if rest.starts_with('/') {
        rest
    } else {
        // Skip the authority (`file://host/path` → `/path`).
        rest.find('/').map(|at| &rest[at..])?
    };
    let raw = path_part.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while let Some(&b) = raw.get(i) {
        if b == b'%' {
            let hi = raw.get(i + 1).copied().and_then(hex_value)?;
            let lo = raw.get(i + 2).copied().and_then(hex_value)?;
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok().map(PathBuf::from)
}

/// One hex digit's value, case-insensitive.
fn hex_value(b: u8) -> Option<u8> {
    char::from(b)
        .to_digit(16)
        .and_then(|v| u8::try_from(v).ok())
}

// ---------------------------------------------------------------------------
// Framing — LSP's base protocol: `Content-Length: N\r\n\r\n<N JSON bytes>`.
// ---------------------------------------------------------------------------

/// Refuse frames a server claims are larger than this (64 MiB) — an honest
/// error beats an unbounded allocation on a misbehaving peer.
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// Write one framed message: the `Content-Length` header pair, then the body.
fn write_frame(writer: &mut impl Write, body: &[u8]) -> io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(body)?;
    writer.flush()
}

/// Read one framed message. `Ok(None)` is a clean end-of-stream (the server
/// closed stdout); unknown headers (e.g. `Content-Type`) are skipped.
fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            // EOF: clean between frames, truncated inside a header block.
            return if content_length.is_none() {
                Ok(None)
            } else {
                Err(io::ErrorKind::UnexpectedEof.into())
            };
        }
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break; // the blank line — body follows
        }
        if let Some(value) = header.strip_prefix("Content-Length:") {
            content_length = value.trim().parse::<usize>().ok();
        }
    }
    let Some(len) = content_length else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP frame missing its Content-Length header",
        ));
    };
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP frame exceeds the 64 MiB sanity cap",
        ));
    }
    let mut body = vec![0_u8; len];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

/// Serialize a JSON-RPC envelope. Serializing a `serde_json::Value` cannot
/// fail (string keys only), so the error arm degrades to an empty body.
fn envelope_bytes(envelope: &Value) -> Vec<u8> {
    serde_json::to_vec(envelope).unwrap_or_default()
}

/// A request envelope (`params: None` omits the member, per JSON-RPC).
fn request_frame(id: i64, method: &str, params: Option<Value>) -> Vec<u8> {
    let mut msg = json!({ "jsonrpc": "2.0", "id": id, "method": method });
    if let (Some(p), Some(obj)) = (params, msg.as_object_mut()) {
        obj.insert("params".to_owned(), p);
    }
    envelope_bytes(&msg)
}

/// A notification envelope (no `id`).
fn notification_frame(method: &str, params: Option<Value>) -> Vec<u8> {
    let mut msg = json!({ "jsonrpc": "2.0", "method": method });
    if let (Some(p), Some(obj)) = (params, msg.as_object_mut()) {
        obj.insert("params".to_owned(), p);
    }
    envelope_bytes(&msg)
}

// ---------------------------------------------------------------------------
// The client.
// ---------------------------------------------------------------------------

/// A request we sent and whose response drives a lifecycle step or a UI-driven
/// navigation reply.
enum Pending {
    /// `initialize` — its response triggers `initialized` + the queue flush.
    Initialize,
    /// `shutdown` — its response triggers `exit`.
    Shutdown,
    /// A UI-driven navigation request (EDITOR-LSP-3) — its response is folded
    /// to the matching [`LspReply`] and pushed to the poll inbox.
    Nav(NavKind),
}

/// Which UI-driven request is in flight, so the reader thread folds its response
/// into the right [`LspReply`] variant (EDITOR-LSP-3).
#[derive(Clone, Copy, Debug)]
enum NavKind {
    /// `textDocument/definition`.
    Definition,
    /// `textDocument/references`.
    References,
    /// `textDocument/prepareRename`.
    PrepareRename,
    /// `textDocument/rename`.
    Rename,
    /// `textDocument/formatting`.
    Format,
}

/// State the reader thread and the client handle share under one mutex, so
/// "queue vs send" and "flush the queue" are atomic with the state flip.
struct Inner {
    /// The observable lifecycle state.
    state: LspState,
    /// Notifications issued while `Initializing`, flushed on `Running`.
    preinit: Vec<Vec<u8>>,
}

/// Everything the I/O threads share with the [`LspClient`] handle.
struct Shared {
    inner: Mutex<Inner>,
    /// path → the latest published diagnostics (replaced wholesale per
    /// `publishDiagnostics`, per the LSP contract).
    diags: Mutex<HashMap<PathBuf, Vec<Diagnostic>>>,
    /// Bumped on every diagnostics change — a cheap repaint/recache signal.
    diag_epoch: AtomicU64,
    /// In-flight requests (lifecycle + navigation) by JSON-RPC id.
    pending: Mutex<HashMap<i64, Pending>>,
    /// The server process, until reaped (reader thread or `Drop`).
    child: Mutex<Option<Child>>,
    /// Completed UI-driven navigation replies (EDITOR-LSP-3), drained by the
    /// panel each frame via [`LspClient::take_replies`].
    replies: Mutex<Vec<LspReply>>,
    /// Bumped on every reply pushed — the cheap poll gate the panel reads via
    /// [`LspClient::replies_epoch`] before it locks the inbox.
    reply_epoch: AtomicU64,
}

impl Shared {
    fn new(state: LspState, child: Option<Child>) -> Self {
        Self {
            inner: Mutex::new(Inner {
                state,
                preinit: Vec::new(),
            }),
            diags: Mutex::new(HashMap::new()),
            diag_epoch: AtomicU64::new(0),
            pending: Mutex::new(HashMap::new()),
            child: Mutex::new(child),
            replies: Mutex::new(Vec::new()),
            reply_epoch: AtomicU64::new(0),
        }
    }

    /// Kill + reap the child if it is still ours to reap.
    fn kill_child(&self) {
        let taken = lock_unpoisoned(&self.child).take();
        if let Some(mut child) = taken {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Lock a mutex, recovering the data from a poisoned lock (a panicked I/O
/// thread must not wedge the UI) — the `mde-term-egui` PTY idiom.
fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One language-server session: process + handshake + document sync + the
/// folded diagnostics store.
///
/// Construction never fails — a missing binary, unregistered language, or
/// spawn error yields a client parked in the matching honest [`LspState`]
/// (§7), which the UI reads via [`LspClient::state`]. All methods are
/// non-blocking.
pub struct LspClient {
    /// The language this session serves.
    language: Language,
    /// State shared with the I/O threads.
    shared: Arc<Shared>,
    /// Frames → the writer thread (`None` when no server was spawned).
    tx: Option<Sender<Vec<u8>>>,
    /// Per-document sync version counters (full-text sync, v1).
    versions: Mutex<HashMap<PathBuf, i32>>,
    /// The next JSON-RPC request id (1 is `initialize`).
    next_id: AtomicI64,
}

impl LspClient {
    /// Start the registered server for `language`, rooted at the (absolute)
    /// workspace `root`.
    ///
    /// Probes `PATH` first: an absent binary parks the client in
    /// [`LspState::Unavailable`] without spawning anything.
    #[must_use]
    pub fn start(language: Language, root: &Path) -> Self {
        Self::start_with_lookup(language, root, find_in_path)
    }

    /// [`Self::start`] with an injectable binary lookup (unit-tested without
    /// touching the process environment).
    fn start_with_lookup(
        language: Language,
        root: &Path,
        lookup: impl Fn(&str) -> Option<PathBuf>,
    ) -> Self {
        let Some(spec) = server_spec(language) else {
            return Self::gated(language, LspState::NoServer { language });
        };
        let Some(resolved) = lookup(spec.program) else {
            return Self::gated(
                language,
                LspState::Unavailable {
                    language,
                    cmd: spec.program.to_owned(),
                },
            );
        };
        Self::start_command(language, resolved, spec.args, root)
    }

    /// Spawn an explicit server command (the registry bypass — also how the
    /// tests drive a fake `sh` server, and how a future per-operator server
    /// override would plug in).
    #[must_use]
    pub fn start_command(
        language: Language,
        program: impl AsRef<OsStr>,
        args: &[&str],
        root: &Path,
    ) -> Self {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Servers chat on stderr; /dev/null keeps a full pipe from ever
            // blocking one (we have no log surface for it yet).
            .stderr(Stdio::null());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                return Self::gated(
                    language,
                    LspState::Failed {
                        reason: format!("failed to spawn the language server: {e}"),
                    },
                )
            }
        };
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Self::gated(
                language,
                LspState::Failed {
                    reason: "the spawned language server exposed no stdio pipes".to_owned(),
                },
            );
        };

        let shared = Arc::new(Shared::new(LspState::Initializing, Some(child)));
        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        // `initialize` (id 1). The pending entry is registered *before* the
        // reader thread exists, so even an instantly-replying server finds it.
        lock_unpoisoned(&shared.pending).insert(1, Pending::Initialize);
        let _ = tx.send(request_frame(
            1,
            "initialize",
            Some(initialize_params(root)),
        ));

        if !spawn_io_threads(&shared, &tx, stdin, stdout, rx) {
            shared.kill_child();
            lock_unpoisoned(&shared.inner).state = LspState::Failed {
                reason: "failed to spawn the LSP client I/O threads".to_owned(),
            };
            return Self {
                language,
                shared,
                tx: None,
                versions: Mutex::new(HashMap::new()),
                next_id: AtomicI64::new(2),
            };
        }

        Self {
            language,
            shared,
            tx: Some(tx),
            versions: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(2),
        }
    }

    /// A client parked in a terminal gated state, with no process behind it.
    fn gated(language: Language, state: LspState) -> Self {
        Self {
            language,
            shared: Arc::new(Shared::new(state, None)),
            tx: None,
            versions: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(2),
        }
    }

    /// The language this client serves.
    #[must_use]
    pub const fn language(&self) -> Language {
        self.language
    }

    /// The current lifecycle state (cloned snapshot).
    #[must_use]
    pub fn state(&self) -> LspState {
        lock_unpoisoned(&self.shared.inner).state.clone()
    }

    /// Document opened in the editor: sends `textDocument/didOpen` (version
    /// 1, full text). The panel calls this when a buffer opens.
    pub fn on_open(&self, path: &Path, text: &str) {
        if self.tx.is_none() {
            return; // gated (§7): nothing to sync to
        }
        let Some(uri) = uri_for(path) else { return };
        lock_unpoisoned(&self.versions).insert(path.to_owned(), 1);
        let params = lsp_types::DidOpenTextDocumentParams {
            text_document: lsp_types::TextDocumentItem {
                uri,
                language_id: Language::from_path(path)
                    .unwrap_or(self.language)
                    .lsp_id()
                    .to_owned(),
                version: 1,
                text: text.to_owned(),
            },
        };
        self.notify("textDocument/didOpen", serde_json::to_value(params).ok());
    }

    /// Document edited: sends `textDocument/didChange` with the **full new
    /// text** (full-text sync, v1) and the bumped version counter. A no-op
    /// for documents never opened via [`Self::on_open`].
    pub fn on_change(&self, path: &Path, text: &str) {
        if self.tx.is_none() {
            return;
        }
        let Some(uri) = uri_for(path) else { return };
        let mut versions = lock_unpoisoned(&self.versions);
        let Some(entry) = versions.get_mut(path) else {
            return; // never opened — LSP requires didOpen first
        };
        *entry += 1;
        let version = *entry;
        drop(versions);
        let params = lsp_types::DidChangeTextDocumentParams {
            text_document: lsp_types::VersionedTextDocumentIdentifier { uri, version },
            content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.to_owned(),
            }],
        };
        self.notify("textDocument/didChange", serde_json::to_value(params).ok());
    }

    /// Document closed: sends `textDocument/didClose`, drops the version
    /// counter, and clears the document's folded diagnostics (a closed
    /// buffer must not show a stale gutter).
    pub fn on_close(&self, path: &Path) {
        if self.tx.is_none() {
            return;
        }
        if lock_unpoisoned(&self.versions).remove(path).is_none() {
            return;
        }
        if lock_unpoisoned(&self.shared.diags).remove(path).is_some() {
            self.shared.diag_epoch.fetch_add(1, Ordering::Relaxed);
        }
        let Some(uri) = uri_for(path) else { return };
        let params = lsp_types::DidCloseTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
        };
        self.notify("textDocument/didClose", serde_json::to_value(params).ok());
    }

    /// The latest published diagnostics for `path` (empty when none), sorted
    /// by position.
    #[must_use]
    pub fn diagnostics_for(&self, path: &Path) -> Vec<Diagnostic> {
        lock_unpoisoned(&self.shared.diags)
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    /// Every document currently holding diagnostics (a problems-panel feed).
    #[must_use]
    pub fn all_diagnostics(&self) -> Vec<(PathBuf, Vec<Diagnostic>)> {
        lock_unpoisoned(&self.shared.diags)
            .iter()
            .map(|(path, diags)| (path.clone(), diags.clone()))
            .collect()
    }

    /// A counter bumped on every diagnostics change — the UI compares it to
    /// a remembered value to skip re-deriving gutter caches on quiet frames.
    #[must_use]
    pub fn diagnostics_epoch(&self) -> u64 {
        self.shared.diag_epoch.load(Ordering::Relaxed)
    }

    /// Begin the graceful `shutdown`/`exit` sequence. Non-blocking: the
    /// reader thread sends `exit` when the server acknowledges, and the
    /// state becomes [`LspState::Stopped`]. A no-op unless the session is
    /// live ([`Drop`] hard-kills whatever remains regardless).
    pub fn shutdown(&self) {
        let Some(tx) = &self.tx else { return };
        let live = matches!(
            lock_unpoisoned(&self.shared.inner).state,
            LspState::Running | LspState::Initializing
        );
        if !live {
            return;
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        lock_unpoisoned(&self.shared.pending).insert(id, Pending::Shutdown);
        let _ = tx.send(request_frame(id, "shutdown", None));
    }

    // ── EDITOR-LSP-3: the UI-driven navigation requests ──────────────────────

    /// Request `textDocument/definition` for the caret at zero-based `line` /
    /// UTF-16 `character`. Returns `true` when a request was dispatched (a live
    /// or initializing session); `false` is the honest gated no-op (§7 — an
    /// absent / dead server enqueues nothing and produces no reply). The reply
    /// arrives asynchronously as [`LspReply::Definition`] on [`Self::take_replies`].
    #[must_use]
    pub fn goto_definition(&self, path: &Path, line: u32, character: u32) -> bool {
        let Some(uri) = path_to_file_uri(path) else {
            return false;
        };
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        });
        self.send_request("textDocument/definition", params, NavKind::Definition)
    }

    /// Request `textDocument/references` (including the declaration) for the
    /// caret. The reply arrives as [`LspReply::References`]. Returns whether a
    /// request was dispatched (see [`Self::goto_definition`]).
    #[must_use]
    pub fn find_references(&self, path: &Path, line: u32, character: u32) -> bool {
        let Some(uri) = path_to_file_uri(path) else {
            return false;
        };
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "context": { "includeDeclaration": true },
        });
        self.send_request("textDocument/references", params, NavKind::References)
    }

    /// Request `textDocument/prepareRename` for the caret — the pre-flight that
    /// yields the renameable span + placeholder ([`LspReply::PrepareRename`]),
    /// used to prefill + validate the rename box. Returns whether a request was
    /// dispatched.
    #[must_use]
    pub fn prepare_rename(&self, path: &Path, line: u32, character: u32) -> bool {
        let Some(uri) = path_to_file_uri(path) else {
            return false;
        };
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        });
        self.send_request(
            "textDocument/prepareRename",
            params,
            NavKind::PrepareRename,
        )
    }

    /// Request `textDocument/rename` of the symbol at the caret to `new_name`.
    /// The reply arrives as [`LspReply::Rename`] carrying the cross-file
    /// [`WorkspaceEdit`] to apply. Returns whether a request was dispatched.
    #[must_use]
    pub fn rename(&self, path: &Path, line: u32, character: u32, new_name: &str) -> bool {
        let Some(uri) = path_to_file_uri(path) else {
            return false;
        };
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "newName": new_name,
        });
        self.send_request("textDocument/rename", params, NavKind::Rename)
    }

    /// Request `textDocument/formatting` (four-space, spaces) for the whole
    /// document. The reply arrives as [`LspReply::Format`] carrying the edits to
    /// apply to the buffer. Returns whether a request was dispatched.
    #[must_use]
    pub fn format(&self, path: &Path) -> bool {
        let Some(uri) = path_to_file_uri(path) else {
            return false;
        };
        let params = json!({
            "textDocument": { "uri": uri },
            "options": { "tabSize": 4, "insertSpaces": true },
        });
        self.send_request("textDocument/formatting", params, NavKind::Format)
    }

    /// Register + dispatch one navigation request, mirroring
    /// [`Self::send_or_queue`]'s "queue-vs-send atomic with the state" discipline
    /// so a request issued during the handshake fires after `initialized`.
    ///
    /// The pending entry is registered *before* the inner lock (never held across
    /// it) so it can't deadlock with the reader thread's pending→inner response
    /// path; a terminal/gated state cleans the speculative entry back up and
    /// returns `false` (the honest no-op).
    fn send_request(&self, method: &str, params: Value, kind: NavKind) -> bool {
        let Some(tx) = &self.tx else {
            return false; // gated (§7): no server to ask
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        lock_unpoisoned(&self.shared.pending).insert(id, Pending::Nav(kind));
        let frame = request_frame(id, method, Some(params));
        let mut inner = lock_unpoisoned(&self.shared.inner);
        match inner.state {
            LspState::Initializing => {
                inner.preinit.push(frame);
                true
            }
            LspState::Running => {
                drop(inner);
                let _ = tx.send(frame);
                true
            }
            _ => {
                drop(inner);
                lock_unpoisoned(&self.shared.pending).remove(&id);
                false
            }
        }
    }

    /// A counter bumped on every navigation reply pushed — the panel compares it
    /// to a remembered value to skip locking the inbox on quiet frames (the same
    /// epoch-gate as [`Self::diagnostics_epoch`]).
    #[must_use]
    pub fn replies_epoch(&self) -> u64 {
        self.shared.reply_epoch.load(Ordering::Relaxed)
    }

    /// Drain every completed navigation reply (EDITOR-LSP-3) — the panel calls
    /// this once per frame (gated on [`Self::replies_epoch`]) and routes each
    /// reply to its seam (jump / list / workspace-edit / format).
    #[must_use]
    pub fn take_replies(&self) -> Vec<LspReply> {
        let mut replies = lock_unpoisoned(&self.shared.replies);
        std::mem::take(&mut *replies)
    }

    /// Queue a notification: sent immediately when `Running`, parked in the
    /// pre-init queue while `Initializing` (the reader flushes it on ready),
    /// dropped in any gated/terminal state.
    fn notify(&self, method: &str, params: Option<Value>) {
        let Some(params) = params else { return };
        self.send_or_queue(notification_frame(method, Some(params)));
    }

    /// The queue-vs-send decision, atomic with the state under one lock.
    fn send_or_queue(&self, frame: Vec<u8>) {
        let Some(tx) = &self.tx else { return };
        let mut inner = lock_unpoisoned(&self.shared.inner);
        if matches!(inner.state, LspState::Initializing) {
            inner.preinit.push(frame);
            return;
        }
        let running = matches!(inner.state, LspState::Running);
        drop(inner);
        if running {
            let _ = tx.send(frame);
        }
    }

    /// The per-document sync version (tests observe the counter).
    #[cfg(test)]
    fn version_of(&self, path: &Path) -> Option<i32> {
        lock_unpoisoned(&self.versions).get(path).copied()
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // Close the writer channel: the writer thread drains and drops the
        // server's stdin (EOF). Then hard-stop whatever still runs — the
        // graceful path is the explicit `shutdown()` API; Drop must never
        // leave an orphaned server on the host. The detached I/O threads
        // exit on their own once the pipes close.
        self.tx = None;
        self.shared.kill_child();
    }
}

/// Spawn the named reader + writer threads; `false` if the OS refused.
fn spawn_io_threads(
    shared: &Arc<Shared>,
    tx: &Sender<Vec<u8>>,
    stdin: ChildStdin,
    stdout: ChildStdout,
    rx: Receiver<Vec<u8>>,
) -> bool {
    let writer = thread::Builder::new()
        .name("mde-editor-lsp-write".into())
        .spawn(move || writer_loop(stdin, &rx));
    let reader = thread::Builder::new()
        .name("mde-editor-lsp-read".into())
        .spawn({
            let shared = Arc::clone(shared);
            let tx = tx.clone();
            move || reader_loop(stdout, &shared, &tx)
        });
    writer.is_ok() && reader.is_ok()
}

/// The `initialize` request params: workspace root, our identity, and (v1)
/// default client capabilities.
fn initialize_params(root: &Path) -> Value {
    let root_uri = path_to_file_uri(root).and_then(|uri| uri.parse::<lsp_types::Uri>().ok());
    let workspace_folders = root_uri.map(|uri| {
        vec![lsp_types::WorkspaceFolder {
            uri,
            name: root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("workspace")
                .to_owned(),
        }]
    });
    let params = lsp_types::InitializeParams {
        process_id: Some(std::process::id()),
        capabilities: lsp_types::ClientCapabilities::default(),
        client_info: Some(lsp_types::ClientInfo {
            name: "mde-editor-egui".to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
        workspace_folders,
        ..lsp_types::InitializeParams::default()
    };
    serde_json::to_value(params).unwrap_or_else(|_| json!({}))
}

/// The LSP `Uri` for a local path, when representable.
fn uri_for(path: &Path) -> Option<lsp_types::Uri> {
    path_to_file_uri(path).and_then(|uri| uri.parse().ok())
}

// ---------------------------------------------------------------------------
// The I/O threads.
// ---------------------------------------------------------------------------

/// The writer pump: frames from the channel → the server's stdin. Exits when
/// the channel closes (client dropped) or the pipe breaks (server died).
fn writer_loop(stdin: ChildStdin, rx: &Receiver<Vec<u8>>) {
    let mut writer = BufWriter::new(stdin);
    while let Ok(body) = rx.recv() {
        if write_frame(&mut writer, &body).is_err() {
            break;
        }
    }
}

/// The reader pump: parse server frames and dispatch until the stream ends,
/// then settle the final state and reap the child.
fn reader_loop(stdout: ChildStdout, shared: &Arc<Shared>, tx: &Sender<Vec<u8>>) {
    let mut reader = BufReader::new(stdout);
    // `Ok(None)` (clean EOF) and `Err` (broken stream) both end the pump.
    while let Ok(Some(bytes)) = read_frame(&mut reader) {
        dispatch(shared, tx, &bytes);
    }
    {
        let mut inner = lock_unpoisoned(&shared.inner);
        if inner.state != LspState::Stopped {
            inner.state = LspState::Failed {
                reason: "the language server closed its stream unexpectedly".to_owned(),
            };
        }
        inner.preinit.clear();
    }
    reap(shared);
}

/// Reap the server after its stream closed: give it a moment to exit on its
/// own (the normal post-`exit` case), then kill. Bounded — runs on the
/// reader thread, never the UI.
fn reap(shared: &Shared) {
    let Some(mut child) = lock_unpoisoned(&shared.child).take() else {
        return;
    };
    for _ in 0..20 {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Route one inbound message: response / notification / server request.
fn dispatch(shared: &Shared, tx: &Sender<Vec<u8>>, bytes: &[u8]) {
    let Ok(msg) = serde_json::from_slice::<Value>(bytes) else {
        return; // unparseable frame — skip, keep the session alive
    };
    let method = msg.get("method").and_then(Value::as_str);
    match (method, msg.get("id")) {
        // A server→client *request*: reply MethodNotFound honestly so the
        // server never stalls awaiting a response we'd otherwise swallow.
        (Some(_), Some(id)) => {
            let reply = json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "not handled by the mde editor LSP client" },
            });
            let _ = tx.send(envelope_bytes(&reply));
        }
        (Some("textDocument/publishDiagnostics"), None) => {
            fold_diagnostics(shared, msg.get("params"));
        }
        // A response to one of our requests.
        (None, Some(_)) => handle_response(shared, tx, &msg),
        // Other notifications (progress, logMessage, …) and id-less noise:
        // ignored gracefully.
        (_, None) => {}
    }
}

/// Fold a `publishDiagnostics` notification into the typed store. The
/// published set *replaces* the document's diagnostics wholesale (the LSP
/// contract — an empty list clears them).
fn fold_diagnostics(shared: &Shared, params: Option<&Value>) {
    let Some(params) = params else { return };
    let Some(path) = params
        .get("uri")
        .and_then(Value::as_str)
        .and_then(file_uri_to_path)
    else {
        return;
    };
    let list = params
        .get("diagnostics")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let Ok(raw) = serde_json::from_value::<Vec<lsp_types::Diagnostic>>(list) else {
        return;
    };
    let mut folded: Vec<Diagnostic> = raw.into_iter().map(Diagnostic::from_lsp).collect();
    folded.sort_by_key(|d| (d.start_line, d.start_character));
    lock_unpoisoned(&shared.diags).insert(path, folded);
    shared.diag_epoch.fetch_add(1, Ordering::Relaxed);
}

/// Handle a response to one of our lifecycle requests.
fn handle_response(shared: &Shared, tx: &Sender<Vec<u8>>, msg: &Value) {
    let Some(id) = msg.get("id").and_then(Value::as_i64) else {
        return;
    };
    let Some(pending) = lock_unpoisoned(&shared.pending).remove(&id) else {
        return; // not ours / already settled
    };
    match pending {
        Pending::Initialize => {
            if let Some(error) = msg.get("error") {
                let reason = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the server rejected initialize")
                    .to_owned();
                let mut inner = lock_unpoisoned(&shared.inner);
                inner.state = LspState::Failed { reason };
                inner.preinit.clear();
                drop(inner);
                return;
            }
            // Handshake done: `initialized`, then flush everything queued
            // behind it — atomically with the state flip.
            let _ = tx.send(notification_frame("initialized", Some(json!({}))));
            let mut inner = lock_unpoisoned(&shared.inner);
            inner.state = LspState::Running;
            for frame in inner.preinit.drain(..) {
                let _ = tx.send(frame);
            }
        }
        Pending::Shutdown => {
            // Acknowledged (result or error — either way we're leaving):
            // `exit` tells the server to terminate; the reader sees EOF next.
            let _ = tx.send(notification_frame("exit", None));
            lock_unpoisoned(&shared.inner).state = LspState::Stopped;
        }
        Pending::Nav(kind) => {
            // A UI-driven request completed: fold the result (honestly empty on
            // an error / null reply — never a fabricated one, §7) and hand it to
            // the panel's poll inbox.
            let reply = fold_reply(kind, msg);
            lock_unpoisoned(&shared.replies).push(reply);
            shared.reply_epoch.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// Navigation response parsing (EDITOR-LSP-3) — pure `serde_json::Value` folds,
// unit-tested with canned JSON (no server). Positions stay on the LSP wire
// (zero-based line, UTF-16 `character`); a malformed / absent field drops that
// element rather than fabricating one.
// ---------------------------------------------------------------------------

/// Fold one navigation response into its [`LspReply`]. An `error` reply (or a
/// missing/`null` `result`) folds to the empty result for its kind.
fn fold_reply(kind: NavKind, msg: &Value) -> LspReply {
    let null = Value::Null;
    let result = msg.get("result").unwrap_or(&null);
    match kind {
        NavKind::Definition => LspReply::Definition(parse_locations(result)),
        NavKind::References => LspReply::References(parse_locations(result)),
        NavKind::PrepareRename => LspReply::PrepareRename(parse_prepare_rename(result)),
        NavKind::Rename => LspReply::Rename(parse_workspace_edit(result)),
        NavKind::Format => LspReply::Format(parse_text_edits(result)),
    }
}

/// A zero-based `{ line, character }` position, or `None` for a malformed one.
fn parse_position(v: &Value) -> Option<(u32, u32)> {
    let line = u32::try_from(v.get("line")?.as_u64()?).ok()?;
    let character = u32::try_from(v.get("character")?.as_u64()?).ok()?;
    Some((line, character))
}

/// A `{ start, end }` range, or `None` when either endpoint is malformed.
fn parse_range(v: &Value) -> Option<LspRange> {
    let (start_line, start_character) = parse_position(v.get("start")?)?;
    let (end_line, end_character) = parse_position(v.get("end")?)?;
    Some(LspRange {
        start_line,
        start_character,
        end_line,
        end_character,
    })
}

/// One `Location` (`{ uri, range }`) or `LocationLink`
/// (`{ targetUri, targetSelectionRange | targetRange }`) — the two shapes a
/// definition / references response element can take.
fn parse_location(v: &Value) -> Option<Location> {
    if let Some(uri) = v.get("uri").and_then(Value::as_str) {
        let path = file_uri_to_path(uri)?;
        let range = parse_range(v.get("range")?)?;
        return Some(Location { path, range });
    }
    // A LocationLink: prefer the identifier span (`targetSelectionRange`),
    // falling back to the whole `targetRange`.
    let uri = v.get("targetUri").and_then(Value::as_str)?;
    let path = file_uri_to_path(uri)?;
    let range_value = v
        .get("targetSelectionRange")
        .or_else(|| v.get("targetRange"))?;
    let range = parse_range(range_value)?;
    Some(Location { path, range })
}

/// Fold a definition / references result into flat [`Location`]s: an array of
/// `Location`/`LocationLink`, a single `Location` object, or `null` → empty.
fn parse_locations(result: &Value) -> Vec<Location> {
    match result {
        Value::Array(items) => items.iter().filter_map(parse_location).collect(),
        Value::Object(_) => parse_location(result).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// One `TextEdit` (`{ range, newText }`), or `None` when malformed.
fn parse_text_edit(v: &Value) -> Option<TextEdit> {
    let range = parse_range(v.get("range")?)?;
    let new_text = v.get("newText")?.as_str()?.to_owned();
    Some(TextEdit { range, new_text })
}

/// Fold a formatting result (an array of `TextEdit`, or `null`) into flat edits.
fn parse_text_edits(result: &Value) -> Vec<TextEdit> {
    match result {
        Value::Array(items) => items.iter().filter_map(parse_text_edit).collect(),
        _ => Vec::new(),
    }
}

/// Fold a rename result into a [`WorkspaceEdit`], preferring the ordered,
/// versioned `documentChanges` and falling back to the `changes` map. Resource
/// operations (create / rename / delete files) are honestly skipped — this
/// applies text edits, and a create/delete has no text to apply.
fn parse_workspace_edit(result: &Value) -> WorkspaceEdit {
    let mut changes: Vec<(PathBuf, Vec<TextEdit>)> = Vec::new();
    if let Some(doc_changes) = result.get("documentChanges").and_then(Value::as_array) {
        for dc in doc_changes {
            let Some(uri) = dc
                .get("textDocument")
                .and_then(|td| td.get("uri"))
                .and_then(Value::as_str)
            else {
                continue; // a resource op — no text edits to apply
            };
            let Some(path) = file_uri_to_path(uri) else {
                continue;
            };
            let edits = dc.get("edits").map(parse_text_edits).unwrap_or_default();
            if !edits.is_empty() {
                changes.push((path, edits));
            }
        }
        return WorkspaceEdit { changes };
    }
    if let Some(map) = result.get("changes").and_then(Value::as_object) {
        for (uri, edits) in map {
            if let Some(path) = file_uri_to_path(uri) {
                let edits = parse_text_edits(edits);
                if !edits.is_empty() {
                    changes.push((path, edits));
                }
            }
        }
    }
    WorkspaceEdit { changes }
}

/// Fold a `prepareRename` result: a `{ range, placeholder }` object, a bare
/// `Range`, or `null` / `{ defaultBehavior }` → `None` (the UI then falls back
/// to the word under the cursor).
fn parse_prepare_rename(result: &Value) -> Option<PrepareRename> {
    if let Some(placeholder) = result.get("placeholder").and_then(Value::as_str) {
        let range = parse_range(result.get("range")?)?;
        return Some(PrepareRename {
            range,
            placeholder: Some(placeholder.to_owned()),
        });
    }
    let range = parse_range(result)?;
    Some(PrepareRename {
        range,
        placeholder: None,
    })
}

// ---------------------------------------------------------------------------
// Tests — a FAKE language server (a tiny `sh` script speaking real LSP
// framing on stdio) proves the handshake, the didOpen → publishDiagnostics
// fold, and the shutdown sequence without any real server installed (§7).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// The fake server: parses real `Content-Length` frames from stdin and
    /// answers with canned JSON-RPC — replies to `initialize` (echoing the
    /// request id) then fires an unsolicited server→client *request* (which
    /// the client must answer, not swallow), publishes one diagnostic for
    /// whatever URI `didOpen` names, acknowledges `shutdown`, and exits on
    /// `exit`.
    const FAKE_SERVER: &str = r#"
emit() {
    printf 'Content-Length: %s\r\n\r\n%s' "$(printf '%s' "$1" | wc -c)" "$1"
}
while :; do
    len=""
    while IFS= read -r line; do
        line=$(printf '%s' "$line" | tr -d '\r')
        [ -z "$line" ] && break
        case "$line" in
            Content-Length:*) len=$(printf '%s' "$line" | sed 's/Content-Length:[[:space:]]*//') ;;
        esac
    done
    [ -z "$len" ] && exit 0
    body=$(head -c "$len")
    case "$body" in
        *'"method":"initialize"'*)
            id=$(printf '%s' "$body" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
            emit '{"jsonrpc":"2.0","id":'"$id"',"result":{"capabilities":{}}}'
            emit '{"jsonrpc":"2.0","id":900,"method":"workspace/configuration","params":{"items":[]}}'
            ;;
        *'"method":"textDocument/didOpen"'*)
            uri=$(printf '%s' "$body" | sed -n 's/.*"uri":"\([^"]*\)".*/\1/p')
            emit '{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"'"$uri"'","diagnostics":[{"range":{"start":{"line":2,"character":4},"end":{"line":2,"character":9}},"severity":2,"message":"fake warning","source":"fake-ls"}]}}'
            ;;
        *'"method":"shutdown"'*)
            id=$(printf '%s' "$body" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
            emit '{"jsonrpc":"2.0","id":'"$id"',"result":null}'
            ;;
        *'"method":"exit"'*)
            exit 0
            ;;
    esac
done
"#;

    /// Spawn a client over the fake server.
    fn fake_client(root: &Path) -> LspClient {
        LspClient::start_command(Language::Rust, "sh", &["-c", FAKE_SERVER], root)
    }

    /// Poll `cond` (the subsystem is asynchronous by design) with a hard
    /// deadline so a regression fails fast instead of hanging the suite.
    fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if cond() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(cond(), "timed out waiting for {what}");
    }

    #[test]
    fn framing_round_trips() {
        let mut wire = Vec::new();
        write_frame(&mut wire, br#"{"a":1}"#).expect("write frame");
        write_frame(&mut wire, b"hello").expect("write frame");
        let mut reader = io::Cursor::new(wire);
        assert_eq!(
            read_frame(&mut reader).expect("first frame").as_deref(),
            Some(br#"{"a":1}"#.as_slice())
        );
        assert_eq!(
            read_frame(&mut reader).expect("second frame").as_deref(),
            Some(b"hello".as_slice())
        );
        assert_eq!(read_frame(&mut reader).expect("clean eof"), None);
    }

    #[test]
    fn framing_skips_unknown_headers() {
        let wire =
            b"Content-Length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\nhi"
                .to_vec();
        let mut reader = io::Cursor::new(wire);
        assert_eq!(
            read_frame(&mut reader).expect("frame").as_deref(),
            Some(b"hi".as_slice())
        );
    }

    #[test]
    fn framing_rejects_missing_length() {
        let mut reader = io::Cursor::new(b"X-Nope: 1\r\n\r\n{}".to_vec());
        assert!(read_frame(&mut reader).is_err());
    }

    #[test]
    fn file_uri_round_trips() {
        let path = Path::new("/tmp/some dir/h\u{e9}llo.rs");
        let uri = path_to_file_uri(path).expect("uri");
        assert_eq!(uri, "file:///tmp/some%20dir/h%C3%A9llo.rs");
        assert_eq!(file_uri_to_path(&uri).as_deref(), Some(path));
        // Relative paths are not documents LSP can address.
        assert_eq!(path_to_file_uri(Path::new("relative.rs")), None);
        // An authority component is tolerated on the way in.
        assert_eq!(
            file_uri_to_path("file://localhost/x.rs").as_deref(),
            Some(Path::new("/x.rs"))
        );
        assert_eq!(file_uri_to_path("https://example.com/x"), None);
        assert_eq!(file_uri_to_path("file:///bad%zz"), None);
    }

    #[test]
    fn registry_maps_languages_to_servers() {
        assert_eq!(
            server_spec(Language::Rust).map(|s| s.program),
            Some("rust-analyzer")
        );
        assert_eq!(
            server_spec(Language::Python).map(|s| s.program),
            Some("pylsp")
        );
        assert_eq!(
            server_spec(Language::TypeScript).map(|s| s.program),
            Some("typescript-language-server")
        );
        // Markdown is prose — honestly unregistered, not a fake session.
        assert_eq!(server_spec(Language::Markdown), None);
        assert_eq!(Language::Rust.lsp_id(), "rust");
        assert_eq!(Language::Bash.lsp_id(), "shellscript");
    }

    #[test]
    fn probe_requires_an_executable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("fake-ls");
        std::fs::write(&exe, "#!/bin/sh\n").expect("write exe");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let plain = dir.path().join("not-exec");
        std::fs::write(&plain, "").expect("write plain");

        let dirs = || std::iter::once(dir.path().to_owned());
        assert_eq!(find_in_dirs("fake-ls", dirs()), Some(exe));
        assert_eq!(find_in_dirs("not-exec", dirs()), None);
        assert_eq!(find_in_path("mde-no-such-language-server-xyz"), None);
    }

    #[test]
    fn absent_binary_is_typed_unavailable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let client = LspClient::start_with_lookup(Language::Rust, dir.path(), |_| None);
        assert_eq!(
            client.state(),
            LspState::Unavailable {
                language: Language::Rust,
                cmd: "rust-analyzer".to_owned(),
            }
        );
        // Doc sync against the gated client is an honest no-op (§7).
        client.on_open(Path::new("/tmp/x.rs"), "fn main() {}\n");
        assert!(client.diagnostics_for(Path::new("/tmp/x.rs")).is_empty());
    }

    #[test]
    fn unregistered_language_is_no_server() {
        let dir = tempfile::tempdir().expect("tempdir");
        let client = LspClient::start(Language::Markdown, dir.path());
        assert_eq!(
            client.state(),
            LspState::NoServer {
                language: Language::Markdown
            }
        );
    }

    #[test]
    fn lsp_initialize_handshake_reaches_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        let client = fake_client(dir.path());
        wait_until("the initialize handshake", || {
            matches!(client.state(), LspState::Running)
        });
    }

    #[test]
    fn lsp_did_open_folds_published_diagnostics() {
        let dir = tempfile::tempdir().expect("tempdir");
        let client = fake_client(dir.path());
        let file = dir.path().join("main.rs");
        // Issued while still Initializing — proves the pre-init queue flushes
        // after the handshake instead of dropping the open.
        client.on_open(&file, "fn main() {\n    let x = 1;\n}\n");
        wait_until("published diagnostics", || {
            !client.diagnostics_for(&file).is_empty()
        });

        let diags = client.diagnostics_for(&file);
        assert_eq!(diags.len(), 1);
        let d = diags.first().expect("one diagnostic");
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(
            (d.start_line, d.start_character, d.end_line, d.end_character),
            (2, 4, 2, 9)
        );
        assert_eq!(d.message, "fake warning");
        assert_eq!(d.source.as_deref(), Some("fake-ls"));
        assert!(client.diagnostics_epoch() >= 1);

        // The full-sync version counter: open=1, each change bumps, close drops.
        assert_eq!(client.version_of(&file), Some(1));
        client.on_change(&file, "fn main() {}\n");
        assert_eq!(client.version_of(&file), Some(2));
        client.on_change(&file, "fn main() { }\n");
        assert_eq!(client.version_of(&file), Some(3));
        client.on_close(&file);
        assert_eq!(client.version_of(&file), None);
        // Closing also clears the stale gutter feed.
        assert!(client.diagnostics_for(&file).is_empty());
    }

    #[test]
    fn lsp_shutdown_exits_the_server() {
        let dir = tempfile::tempdir().expect("tempdir");
        let client = fake_client(dir.path());
        wait_until("the initialize handshake", || {
            matches!(client.state(), LspState::Running)
        });
        client.shutdown();
        wait_until("the shutdown handshake", || {
            matches!(client.state(), LspState::Stopped)
        });
    }

    #[test]
    fn lsp_dead_server_is_failed_not_fake_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A "server" that exits immediately: the stream closes during the
        // handshake and the state must settle on Failed — never Running.
        let client = LspClient::start_command(Language::Rust, "sh", &["-c", "exit 0"], dir.path());
        wait_until("the failure state", || {
            matches!(client.state(), LspState::Failed { .. })
        });
    }

    #[test]
    fn lsp_unspawnable_command_is_failed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let client = LspClient::start_command(
            Language::Rust,
            "/nonexistent/mde-editor-lsp-test-binary",
            &[],
            dir.path(),
        );
        assert!(matches!(client.state(), LspState::Failed { .. }));
    }

    // ── EDITOR-LSP-3: navigation response parsing (pure, no server) ───────────

    #[test]
    fn parse_locations_folds_object_array_and_location_links() {
        // A single Location object.
        let single = json!({
            "uri": "file:///tmp/x.rs",
            "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 6 } },
        });
        let locs = parse_locations(&single);
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].path, PathBuf::from("/tmp/x.rs"));
        assert_eq!(locs[0].range.start_line, 1);
        assert_eq!(locs[0].range.end_character, 6);

        // An array mixing a Location and a LocationLink (which uses
        // `targetUri` + `targetSelectionRange`).
        let arr = json!([
            { "uri": "file:///tmp/a.rs", "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } } },
            { "targetUri": "file:///tmp/b.rs",
              "targetRange": { "start": { "line": 9, "character": 0 }, "end": { "line": 12, "character": 1 } },
              "targetSelectionRange": { "start": { "line": 9, "character": 4 }, "end": { "line": 9, "character": 7 } } },
        ]);
        let locs = parse_locations(&arr);
        assert_eq!(locs.len(), 2);
        assert_eq!(locs[1].path, PathBuf::from("/tmp/b.rs"));
        // The link folds to the *selection* range (the identifier), not the whole range.
        assert_eq!(
            (locs[1].range.start_line, locs[1].range.start_character),
            (9, 4)
        );
        // A null result is honestly empty — no fabricated location (§7).
        assert!(parse_locations(&Value::Null).is_empty());
    }

    #[test]
    fn parse_workspace_edit_reads_changes_and_document_changes() {
        // The `changes` map shape → one entry per file.
        let map = json!({
            "changes": {
                "file:///tmp/one.rs": [
                    { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "newText": "bar" }
                ],
                "file:///tmp/two.rs": [
                    { "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 3 } }, "newText": "bar" }
                ],
            }
        });
        let we = parse_workspace_edit(&map);
        assert_eq!(we.changes.len(), 2, "a cross-file rename touches both files");
        assert!(!we.is_empty());

        // The `documentChanges` shape, with a resource op that carries no text
        // edits (honestly skipped).
        let dc = json!({
            "documentChanges": [
                { "textDocument": { "uri": "file:///tmp/a.rs", "version": 3 },
                  "edits": [ { "range": { "start": { "line": 2, "character": 4 }, "end": { "line": 2, "character": 7 } }, "newText": "baz" } ] },
                { "kind": "create", "uri": "file:///tmp/new.rs" },
            ]
        });
        let we = parse_workspace_edit(&dc);
        assert_eq!(we.changes.len(), 1, "the create op has no text to apply");
        assert_eq!(we.changes[0].0, PathBuf::from("/tmp/a.rs"));
        assert_eq!(we.changes[0].1[0].new_text, "baz");
        // A null rename result is an empty (honest "nothing to rename") edit.
        assert!(parse_workspace_edit(&Value::Null).is_empty());
    }

    #[test]
    fn parse_prepare_rename_handles_each_shape() {
        // `{ range, placeholder }`.
        let full = json!({
            "range": { "start": { "line": 2, "character": 4 }, "end": { "line": 2, "character": 7 } },
            "placeholder": "foo",
        });
        let pr = parse_prepare_rename(&full).expect("a renameable span");
        assert_eq!(pr.placeholder.as_deref(), Some("foo"));
        assert_eq!(pr.range.start_character, 4);

        // A bare Range (no placeholder).
        let bare = json!({ "start": { "line": 0, "character": 1 }, "end": { "line": 0, "character": 5 } });
        let pr = parse_prepare_rename(&bare).expect("a bare range");
        assert!(pr.placeholder.is_none());

        // `null` / `{ defaultBehavior }` → not renameable (fall back to the word).
        assert!(parse_prepare_rename(&Value::Null).is_none());
        assert!(parse_prepare_rename(&json!({ "defaultBehavior": true })).is_none());
    }

    /// A fake server that reaches `Running` then answers each EDITOR-LSP-3
    /// request with a canned result (echoing the request id) — the LSP-1
    /// fake-`sh`-server idiom extended to the navigation requests, so the
    /// request → typed-reply round-trip is proven with no real server (§7).
    const NAV_SERVER: &str = r#"
emit() {
    printf 'Content-Length: %s\r\n\r\n%s' "$(printf '%s' "$1" | wc -c)" "$1"
}
while :; do
    len=""
    while IFS= read -r line; do
        line=$(printf '%s' "$line" | tr -d '\r')
        [ -z "$line" ] && break
        case "$line" in
            Content-Length:*) len=$(printf '%s' "$line" | sed 's/Content-Length:[[:space:]]*//') ;;
        esac
    done
    [ -z "$len" ] && exit 0
    body=$(head -c "$len")
    id=$(printf '%s' "$body" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "$body" in
        *'"method":"initialize"'*)
            emit '{"jsonrpc":"2.0","id":'"$id"',"result":{"capabilities":{}}}'
            ;;
        *'"method":"textDocument/definition"'*)
            emit '{"jsonrpc":"2.0","id":'"$id"',"result":[{"uri":"file:///tmp/def.rs","range":{"start":{"line":3,"character":4},"end":{"line":3,"character":9}}}]}'
            ;;
        *'"method":"textDocument/references"'*)
            emit '{"jsonrpc":"2.0","id":'"$id"',"result":[{"uri":"file:///tmp/a.rs","range":{"start":{"line":1,"character":0},"end":{"line":1,"character":3}}},{"uri":"file:///tmp/b.rs","range":{"start":{"line":5,"character":2},"end":{"line":5,"character":5}}}]}'
            ;;
        *'"method":"textDocument/prepareRename"'*)
            emit '{"jsonrpc":"2.0","id":'"$id"',"result":{"range":{"start":{"line":2,"character":4},"end":{"line":2,"character":7}},"placeholder":"foo"}}'
            ;;
        *'"method":"textDocument/rename"'*)
            emit '{"jsonrpc":"2.0","id":'"$id"',"result":{"changes":{"file:///tmp/one.rs":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},"newText":"bar"}],"file:///tmp/two.rs":[{"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":3}},"newText":"bar"}]}}}'
            ;;
        *'"method":"textDocument/formatting"'*)
            emit '{"jsonrpc":"2.0","id":'"$id"',"result":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"X"}]}'
            ;;
        *'"method":"shutdown"'*)
            emit '{"jsonrpc":"2.0","id":'"$id"',"result":null}'
            ;;
        *'"method":"exit"'*)
            exit 0
            ;;
    esac
done
"#;

    /// Spawn a client over the nav fake server, waited to `Running`.
    fn nav_client(root: &Path) -> LspClient {
        let client = LspClient::start_command(Language::Rust, "sh", &["-c", NAV_SERVER], root);
        wait_until("the initialize handshake", || {
            matches!(client.state(), LspState::Running)
        });
        client
    }

    /// Fire a request, then drain the one reply it produces.
    fn one_reply(client: &LspClient) -> LspReply {
        wait_until("a navigation reply", || client.replies_epoch() >= 1);
        client
            .take_replies()
            .into_iter()
            .next()
            .expect("exactly one reply")
    }

    #[test]
    fn definition_request_round_trips_to_a_location() {
        let dir = tempfile::tempdir().expect("tempdir");
        let client = nav_client(dir.path());
        assert!(client.goto_definition(Path::new("/tmp/src.rs"), 3, 4));
        let LspReply::Definition(locs) = one_reply(&client) else {
            unreachable!("the nav server answers definition with a Definition reply")
        };
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].path, PathBuf::from("/tmp/def.rs"));
        assert_eq!(locs[0].range.start_line, 3);
    }

    #[test]
    fn references_request_round_trips_to_a_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let client = nav_client(dir.path());
        assert!(client.find_references(Path::new("/tmp/src.rs"), 1, 0));
        let LspReply::References(locs) = one_reply(&client) else {
            unreachable!("the nav server answers references with a References reply")
        };
        assert_eq!(locs.len(), 2, "both references are listed");
        assert_eq!(locs[0].path, PathBuf::from("/tmp/a.rs"));
        assert_eq!(locs[1].path, PathBuf::from("/tmp/b.rs"));
    }

    #[test]
    fn rename_request_round_trips_to_a_cross_file_workspace_edit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let client = nav_client(dir.path());
        assert!(client.rename(Path::new("/tmp/src.rs"), 0, 0, "bar"));
        let LspReply::Rename(edit) = one_reply(&client) else {
            unreachable!("the nav server answers rename with a Rename reply")
        };
        assert_eq!(edit.changes.len(), 2, "the rename spans two files");
        let paths: Vec<_> = edit.changes.iter().map(|(p, _)| p.clone()).collect();
        assert!(paths.contains(&PathBuf::from("/tmp/one.rs")));
        assert!(paths.contains(&PathBuf::from("/tmp/two.rs")));
    }

    #[test]
    fn prepare_rename_and_format_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let client = nav_client(dir.path());
        assert!(client.prepare_rename(Path::new("/tmp/src.rs"), 2, 4));
        let LspReply::PrepareRename(Some(pr)) = one_reply(&client) else {
            unreachable!("the nav server answers prepareRename with a placeholder")
        };
        assert_eq!(pr.placeholder.as_deref(), Some("foo"));

        assert!(client.format(Path::new("/tmp/src.rs")));
        wait_until("the format reply", || client.replies_epoch() >= 2);
        let reply = client.take_replies().into_iter().next().expect("a reply");
        let LspReply::Format(edits) = reply else {
            unreachable!("the nav server answers formatting with a Format reply")
        };
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "X");
    }

    #[test]
    fn navigation_on_a_gated_client_is_an_honest_no_op() {
        // §7: an absent server binary parks the client Unavailable — every
        // navigation request is a no-op returning `false`, and NO reply is ever
        // fabricated.
        let dir = tempfile::tempdir().expect("tempdir");
        let client = LspClient::start_with_lookup(Language::Rust, dir.path(), |_| None);
        assert!(matches!(client.state(), LspState::Unavailable { .. }));
        assert!(!client.goto_definition(Path::new("/tmp/x.rs"), 0, 0));
        assert!(!client.find_references(Path::new("/tmp/x.rs"), 0, 0));
        assert!(!client.prepare_rename(Path::new("/tmp/x.rs"), 0, 0));
        assert!(!client.rename(Path::new("/tmp/x.rs"), 0, 0, "y"));
        assert!(!client.format(Path::new("/tmp/x.rs")));
        assert_eq!(client.replies_epoch(), 0);
        assert!(client.take_replies().is_empty());
    }
}
