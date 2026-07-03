//! FILEMGR-7 — the mackesd **peer-side direct-transfer helper**.
//!
//! Design: `docs/design/file-manager-full.md` (lock 16). When the Files surface
//! copies a file from mounted peer **A** to mounted peer **B**, the bytes must
//! move **directly** A→B, not double-hop through the browsing node **C** that
//! mounted both over sshfs. This responder is the mesh-side executor: it serves
//! `action/mesh-transfer/direct`, and for each request drives a **remote-to-
//! remote rsync over the Nebula overlay** — `ssh <A>.mesh "rsync … <B>.mesh:…"`
//! — so C only orchestrates while the file bytes flow A→B.
//!
//! It reuses the FILEMGR-5/6 mesh chain verbatim (§6): the shared, node-sealed
//! SSH key ([`crate::workers::mesh_mount::KeyProvider`] /
//! [`crate::workers::mesh_mount::SecretStoreKeyProvider`]), the `<host>.mesh`
//! overlay DNS ([`crate::workers::mesh_dns::MESH_SUFFIX`]), the mesh login user,
//! and the published mount scope (`state/mesh-mount/<host>`) to resolve each
//! mount-relative path to its remote absolute path.
//!
//! ## §9 / §7 — typed inputs, honest gate, never faked
//!
//! The request body is a typed [`DirectXferRequest`] (hosts + mount-relative
//! paths + mode — never a command string); every path is sanitized against
//! `..`-escape before it reaches the plan. The planning + path-resolution +
//! rsync-stats parsing folds ([`plan_direct`], [`remote_arg`], [`shell_squote`],
//! [`parse_rsync_stats`]) are **pure + unit-tested**. The one leg that needs a
//! live mesh — actually running `ssh`/`rsync` A→B — is behind the injectable
//! [`TransferBackend`] seam; the live [`RsyncSshBackend`] honestly refuses on a
//! box without `ssh` or the provisioned key with a typed [`TransferError::Gated`]
//! (→ the surface falls back to the sshfs relay) and NEVER fabricates a transfer.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use crate::workers::mesh_dns::MESH_SUFFIX;
use crate::workers::mesh_mount::{
    KeyProvider, MountScope, SecretStoreKeyProvider, DEFAULT_MESH_USER,
};

/// Action-topic prefix for the direct-transfer surface (`action/mesh-transfer/<verb>`).
pub const MESH_TRANSFER_PREFIX: &str = "mesh-transfer";

/// The single verb served: a direct A→B transfer.
pub const DIRECT_VERB: &str = "direct";

/// Verbs served on `action/mesh-transfer/<verb>` (for the responder registration).
pub const MESH_TRANSFER_VERBS: [&str; 1] = [DIRECT_VERB];

/// Bounded SSH connect timeout for the orchestration hop (seconds). The transfer
/// itself may run long; only the initial connect is bounded here (matches the
/// mesh_mount tuning).
const CONNECT_TIMEOUT_SECS: u64 = 8;

// ── the typed request ───────────────────────────────────────────────────────

/// The typed body of an `action/mesh-transfer/direct` request (mirrors the
/// `mde-files` `transfer::DirectRequest` producer).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DirectXferRequest {
    /// The queue id the surface assigned (echoed for correlation).
    #[serde(default)]
    pub op_id: u64,
    /// Source peer host.
    pub src_host: String,
    /// Source item, relative to the source mountpoint.
    pub src_rel: String,
    /// Destination peer host.
    pub dst_host: String,
    /// Destination directory, relative to the dest mountpoint.
    pub dst_rel: String,
    /// `"copy"` (default) or `"move"`.
    #[serde(default = "default_mode")]
    pub mode: String,
}

fn default_mode() -> String {
    "copy".to_string()
}

/// Copy vs move for the direct leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XferMode {
    /// Copy the source, leaving it on A.
    Copy,
    /// Move: rsync `--remove-source-files` unlinks each source file on A after
    /// it lands on B (the copy fully succeeds first, so a failure never loses
    /// data). Emptied source directories are left behind — a known rsync-move
    /// limitation surfaced honestly, not worked around silently.
    Move,
}

impl XferMode {
    /// Parse the wire tag; anything but `"move"` is a copy.
    #[must_use]
    pub fn parse(tag: &str) -> Self {
        match tag {
            "move" => Self::Move,
            _ => Self::Copy,
        }
    }
}

// ── validation (pure) ───────────────────────────────────────────────────────

/// A hostname is a single clean DNS-ish component: non-empty, no `/`, no `..`,
/// no NUL, no whitespace (it becomes `<host>.mesh`).
#[must_use]
pub fn is_clean_host(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\0')
        && !s.chars().any(char::is_whitespace)
}

/// Sanitize a mount-relative path: reject an absolute path, any `..` traversal
/// component, a NUL byte, or a control char, and normalize `.`/empty segments
/// away. Returns the cleaned relative path (may be empty = the mount root).
///
/// This is the load-bearing security guard: a Bus writer must never be able to
/// smuggle `../../etc/shadow` into the remote rsync spec.
#[must_use]
pub fn sanitize_rel(rel: &str) -> Option<String> {
    if rel.contains('\0') || rel.chars().any(char::is_control) {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}       // stray `//` or `.` — skip
            ".." => return None, // traversal — refuse outright
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

// ── remote path + plan (pure) ───────────────────────────────────────────────

/// Resolve a mount-relative path to its rsync remote-path argument for a host at
/// `scope`. A **home** mount maps the mountpoint to the login home, so the
/// remote path is relative (`docs/a.txt`); a **full** mount maps it to `/`, so
/// the remote path is absolute (`/etc/x`). An empty rel is `.` (the mount root).
#[must_use]
pub fn remote_arg(scope: MountScope, rel: &str) -> String {
    let rel = rel.trim_start_matches('/');
    match scope {
        MountScope::Home => {
            if rel.is_empty() {
                ".".to_string()
            } else {
                rel.to_string()
            }
        }
        MountScope::Full => format!("/{rel}"),
    }
}

/// A fully-resolved, executable direct-transfer plan — the pure output of
/// [`plan_direct`]. The [`TransferBackend`] seam turns this into the actual
/// `ssh`/`rsync` invocation; nothing here shells out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectPlan {
    /// The orchestration SSH target: `<mesh_user>@<src_host>.mesh` (C connects
    /// here; the rsync runs on A).
    pub ssh_target: String,
    /// The rsync remote command executed **on A**, with paths shell-quoted.
    pub remote_command: String,
    /// The shared mesh SSH identity file.
    pub identity_key: PathBuf,
    /// Copy vs move.
    pub mode: XferMode,
    /// The full `ssh` argv the live backend runs (`ssh <opts> <target> <cmd>`).
    pub argv: Vec<String>,
}

/// POSIX single-quote a string for safe embedding in a remote shell command:
/// wrap in `'…'`, and encode any embedded `'` as `'\''`.
#[must_use]
pub fn shell_squote(s: &str) -> String {
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

/// The shared `ssh` options (used for both the outer orchestration hop + the
/// inner A→B rsync transport): the shared identity, overlay-safe host-key
/// handling, batch mode, and a bounded connect.
fn ssh_opts(identity_key: &Path) -> Vec<String> {
    vec![
        "-i".to_string(),
        identity_key.display().to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),
        "UserKnownHostsFile=/dev/null".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout={CONNECT_TIMEOUT_SECS}"),
    ]
}

/// One end of a direct transfer: the peer host, its live mount scope, and the
/// (already-sanitized) mount-relative path. Bundled so [`plan_direct`] reads as
/// `plan(src → dst)` rather than a long positional argument list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEnd {
    /// Short peer hostname (becomes `<host>.mesh`).
    pub host: String,
    /// The host's live mount scope (home vs full).
    pub scope: MountScope,
    /// The sanitized mount-relative path.
    pub rel: String,
}

impl RemoteEnd {
    /// Construct an end.
    #[must_use]
    pub fn new(host: impl Into<String>, scope: MountScope, rel: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            scope,
            rel: rel.into(),
        }
    }
}

/// Build the (pure) direct-transfer plan. `mesh_user` + `identity_key` come from
/// the FILEMGR-6 shared key; each end's scope comes from that host's live mount
/// state (default home). Callers pass **already-sanitized** rels.
#[must_use]
pub fn plan_direct(
    mesh_user: &str,
    identity_key: &Path,
    src: &RemoteEnd,
    dst: &RemoteEnd,
    mode: XferMode,
) -> DirectPlan {
    let ssh_target = format!("{mesh_user}@{}.{MESH_SUFFIX}", src.host);
    let src_arg = remote_arg(src.scope, &src.rel);
    // rsync places the source basename INTO the dest dir → a trailing slash on
    // the dest so `rsync src dst/` is the "paste here" shape.
    let dst_path = remote_arg(dst.scope, &dst.rel);
    let dst_spec = format!(
        "{mesh_user}@{}.{MESH_SUFFIX}:{}/",
        dst.host,
        dst_path.trim_end_matches('/')
    );

    // The inner ssh transport A→B reuses the same shared key + options. Its
    // value carries no single quotes, so it embeds safely in the remote command.
    let inner_ssh = format!("ssh {}", ssh_opts(identity_key).join(" "));
    let mut rsync = format!(
        "rsync -a --stats --info=progress2 -e {}",
        shell_squote(&inner_ssh)
    );
    if matches!(mode, XferMode::Move) {
        rsync.push_str(" --remove-source-files");
    }
    let remote_command = format!(
        "{rsync} {} {}",
        shell_squote(&src_arg),
        shell_squote(&dst_spec)
    );

    let mut argv = vec!["ssh".to_string()];
    argv.extend(ssh_opts(identity_key));
    argv.push(ssh_target.clone());
    argv.push(remote_command.clone());

    DirectPlan {
        ssh_target,
        remote_command,
        identity_key: identity_key.to_path_buf(),
        mode,
        argv,
    }
}

// ── typed errors + the backend seam ─────────────────────────────────────────

/// A typed direct-transfer failure. `Gated` is the honest headless/unprovisioned
/// refusal (→ the surface relays); `Failed` is a real rsync/ssh error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferError {
    /// Prerequisites absent on this box (no `ssh`, no provisioned mesh key). The
    /// §7 honest gate — never a fabricated transfer.
    Gated(String),
    /// A malformed / unsafe request (bad host, `..` in a path, missing field).
    Rejected(String),
    /// The transfer ran but rsync/ssh failed.
    Failed(String),
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gated(m) => write!(f, "direct transfer gated: {m}"),
            Self::Rejected(m) => write!(f, "direct transfer rejected: {m}"),
            Self::Failed(m) => write!(f, "direct transfer failed: {m}"),
        }
    }
}

impl std::error::Error for TransferError {}

/// What a completed transfer moved (parsed from rsync `--stats`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransferReport {
    /// Regular files transferred.
    pub files: u64,
    /// Total transferred file bytes.
    pub bytes: u64,
}

/// The `ssh`/`rsync` seam (§9 — the only place that shells out). Injectable so
/// the responder's orchestration is tested with a fake and the live impl stays
/// node-gated.
pub trait TransferBackend: Send + Sync {
    /// Run `plan`, returning what it moved. NEVER fakes success.
    ///
    /// # Errors
    /// Any [`TransferError`]; on a headless box it is [`TransferError::Gated`].
    fn run(&self, plan: &DirectPlan) -> Result<TransferReport, TransferError>;
}

/// The live remote-to-remote rsync backend. Integration-only: honestly refuses
/// on a box without `ssh` or the provisioned key with [`TransferError::Gated`]
/// and never fabricates a transfer (§7).
#[derive(Debug, Clone, Default)]
pub struct RsyncSshBackend;

impl RsyncSshBackend {
    /// Construct the live backend.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Preflight the orchestration prerequisites on THIS box: an `ssh` client +
    /// the provisioned shared key. (`rsync` runs on peer A, not here, so its
    /// absence surfaces as a `Failed` from the remote command, not a local gate.)
    fn preflight(plan: &DirectPlan) -> Result<(), TransferError> {
        if !binary_on_path("ssh") {
            return Err(TransferError::Gated("ssh client not found".to_string()));
        }
        if !plan.identity_key.is_file() {
            return Err(TransferError::Gated(format!(
                "mesh SSH key not provisioned at {} (FILEMGR-6)",
                plan.identity_key.display()
            )));
        }
        Ok(())
    }
}

impl TransferBackend for RsyncSshBackend {
    fn run(&self, plan: &DirectPlan) -> Result<TransferReport, TransferError> {
        // Honest gate FIRST — refuse cleanly rather than shell out into a hang.
        Self::preflight(plan)?;
        let mut cmd = Command::new(&plan.argv[0]);
        cmd.args(&plan.argv[1..]);
        let out = cmd
            .output()
            .map_err(|e| TransferError::Failed(format!("spawn ssh: {e}")))?;
        if out.status.success() {
            Ok(parse_rsync_stats(&String::from_utf8_lossy(&out.stdout)))
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(TransferError::Failed(if stderr.is_empty() {
                format!("ssh/rsync exit {:?}", out.status.code())
            } else {
                stderr
            }))
        }
    }
}

/// Parse rsync `--stats` stdout into a [`TransferReport`]. Reads the
/// "Number of regular files transferred" + "Total transferred file size" lines
/// (comma-grouped ints); missing lines contribute zero. Pure + tested.
#[must_use]
pub fn parse_rsync_stats(stdout: &str) -> TransferReport {
    let mut report = TransferReport::default();
    for line in stdout.lines() {
        if let Some(n) = stat_value(line, "Number of regular files transferred:") {
            report.files = n;
        } else if let Some(n) = stat_value(line, "Total transferred file size:") {
            report.bytes = n;
        }
    }
    report
}

/// Extract the first comma-grouped integer following `label` on `line`.
fn stat_value(line: &str, label: &str) -> Option<u64> {
    let rest = line.trim().strip_prefix(label)?;
    let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// `true` when `bin` resolves on `$PATH` (via `which`) — the live gate.
fn binary_on_path(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ── the responder ───────────────────────────────────────────────────────────

/// The direct-transfer responder. Holds the shared-key seam + mesh user + an
/// optional Bus dir (to read each host's mount scope; defaults home when
/// absent). Registered on the Files Bus responder as `action/mesh-transfer/direct`.
pub struct MeshTransfer {
    backend: Arc<dyn TransferBackend>,
    keys: Arc<dyn KeyProvider>,
    mesh_user: String,
    /// Bus data dir for reading `state/mesh-mount/<host>` scope. `None` (or an
    /// unreadable/absent record) → home scope, the least-privilege default.
    bus_dir: Option<PathBuf>,
}

impl MeshTransfer {
    /// Construct with production seams: the shared-key provider materializes the
    /// FILEMGR-6 sealed key at the same path FILEMGR-5's mount worker uses.
    #[must_use]
    pub fn new(runtime_base: PathBuf, repo_dir: PathBuf, workgroup_root: PathBuf) -> Self {
        let key_path = runtime_base.join("mde-mesh").join(".mesh-ssh-key");
        let keys = Arc::new(SecretStoreKeyProvider::new(
            key_path,
            repo_dir,
            workgroup_root,
        ));
        Self {
            backend: Arc::new(RsyncSshBackend::new()),
            keys,
            mesh_user: DEFAULT_MESH_USER.to_string(),
            bus_dir: None,
        }
    }

    /// Inject the transfer backend (tests use a fake).
    #[must_use]
    pub fn with_backend(mut self, backend: Arc<dyn TransferBackend>) -> Self {
        self.backend = backend;
        self
    }

    /// Inject the key provider (tests use a fake).
    #[must_use]
    pub fn with_key_provider(mut self, keys: Arc<dyn KeyProvider>) -> Self {
        self.keys = keys;
        self
    }

    /// Set the Bus data dir used to read live mount scopes.
    #[must_use]
    pub fn with_bus_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.bus_dir = dir;
        self
    }

    /// Override the mesh SSH login user.
    #[must_use]
    pub fn with_mesh_user(mut self, user: impl Into<String>) -> Self {
        self.mesh_user = user.into();
        self
    }

    /// `action/mesh-transfer/<verb>` reply builder (the [`crate::ipc::files::Surface`]
    /// `reply` closure calls this).
    #[must_use]
    pub fn reply(&self, verb: &str, body: Option<&str>) -> String {
        match verb {
            DIRECT_VERB => self.direct_reply(body),
            other => {
                json!({ "ok": false, "error": format!("unknown mesh-transfer verb: {other}") })
                    .to_string()
            }
        }
    }

    /// Handle one `direct` request: parse + validate, resolve scopes + the shared
    /// key, plan, run the backend, and format the typed JSON reply. A
    /// [`TransferError::Gated`] carries `"gated": true` so the surface relays.
    fn direct_reply(&self, body: Option<&str>) -> String {
        let req = match parse_request(body) {
            Ok(r) => r,
            Err(e) => return error_reply(&e),
        };
        let key = match self.keys.identity_key() {
            Ok(k) => k,
            // The mount worker's KeyProvider returns MountError; surface its
            // Gated as our Gated so an unprovisioned key relays honestly.
            Err(e) => {
                return error_reply(&TransferError::Gated(e.to_string()));
            }
        };
        let src = RemoteEnd::new(
            &req.src_host,
            self.scope_of(&req.src_host),
            sanitize_rel(&req.src_rel).unwrap_or_default(),
        );
        let dst = RemoteEnd::new(
            &req.dst_host,
            self.scope_of(&req.dst_host),
            sanitize_rel(&req.dst_rel).unwrap_or_default(),
        );
        let plan = plan_direct(
            &self.mesh_user,
            &key,
            &src,
            &dst,
            XferMode::parse(&req.mode),
        );
        match self.backend.run(&plan) {
            Ok(report) => json!({
                "ok": true,
                "op_id": req.op_id,
                "files": report.files,
                "bytes": report.bytes,
            })
            .to_string(),
            Err(e) => error_reply(&e),
        }
    }

    /// Read `host`'s live mount scope from `state/mesh-mount/<host>`; home when
    /// absent/unreadable (the least-privilege default).
    fn scope_of(&self, host: &str) -> MountScope {
        let Some(dir) = self.bus_dir.as_ref() else {
            return MountScope::Home;
        };
        read_mount_scope(dir, host).unwrap_or(MountScope::Home)
    }
}

/// Parse + validate a request body into a [`DirectXferRequest`]: the JSON must
/// decode, the hosts must be clean + distinct (same-host is a local copy, not a
/// direct transfer), and both rels must be safe relative paths (no `..`).
fn parse_request(body: Option<&str>) -> Result<DirectXferRequest, TransferError> {
    let raw = body.ok_or_else(|| {
        TransferError::Rejected("missing body (need {src_host,src_rel,dst_host,dst_rel})".into())
    })?;
    let req: DirectXferRequest = serde_json::from_str(raw)
        .map_err(|e| TransferError::Rejected(format!("malformed request: {e}")))?;
    if !is_clean_host(&req.src_host) || !is_clean_host(&req.dst_host) {
        return Err(TransferError::Rejected(
            "src_host/dst_host must be a clean hostname".into(),
        ));
    }
    if req.src_host == req.dst_host {
        return Err(TransferError::Rejected(
            "direct transfer needs two distinct peers (same-host is a local copy)".into(),
        ));
    }
    if sanitize_rel(&req.src_rel).is_none() || sanitize_rel(&req.dst_rel).is_none() {
        return Err(TransferError::Rejected(
            "src_rel/dst_rel must be a safe relative path (no `..`)".into(),
        ));
    }
    Ok(req)
}

/// Read the scope tag from the latest `state/mesh-mount/<host>` record. `None`
/// on any miss (no Bus, no record, unparseable, no scope field).
fn read_mount_scope(bus_dir: &Path, host: &str) -> Option<MountScope> {
    let persist = mde_bus::persist::Persist::open(bus_dir.to_path_buf()).ok()?;
    let topic = format!("{}{host}", crate::workers::mesh_mount::STATE_PREFIX);
    let latest = persist.list_since(&topic, None).ok()?.pop()?;
    let body = latest.body?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    match v.get("scope").and_then(serde_json::Value::as_str)? {
        "full" => Some(MountScope::Full),
        _ => Some(MountScope::Home),
    }
}

/// Format a typed error into the JSON reply envelope, tagging `gated` so the
/// producer can distinguish an honest relay-fallback from a hard failure.
fn error_reply(e: &TransferError) -> String {
    json!({
        "ok": false,
        "gated": matches!(e, TransferError::Gated(_)),
        "error": e.to_string(),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::mesh_mount::MountError;
    use std::sync::Mutex;

    // ── validation ───────────────────────────────────────────────────────

    #[test]
    fn clean_host_rejects_traversal_and_slashes() {
        assert!(is_clean_host("oak"));
        assert!(!is_clean_host(""));
        assert!(!is_clean_host(".."));
        assert!(!is_clean_host("a/b"));
        assert!(!is_clean_host("a b"));
    }

    #[test]
    fn sanitize_rel_strips_dots_and_refuses_traversal() {
        assert_eq!(sanitize_rel("docs/a.txt").as_deref(), Some("docs/a.txt"));
        assert_eq!(sanitize_rel("").as_deref(), Some(""));
        assert_eq!(sanitize_rel("./a//b").as_deref(), Some("a/b"));
        assert_eq!(sanitize_rel("/leading").as_deref(), Some("leading"));
        assert!(sanitize_rel("../etc/shadow").is_none());
        assert!(sanitize_rel("a/../../etc").is_none());
        assert!(sanitize_rel("a\0b").is_none());
    }

    // ── remote path resolution ───────────────────────────────────────────

    #[test]
    fn remote_arg_is_relative_for_home_absolute_for_full() {
        assert_eq!(remote_arg(MountScope::Home, "docs/a.txt"), "docs/a.txt");
        assert_eq!(remote_arg(MountScope::Home, ""), ".");
        assert_eq!(remote_arg(MountScope::Full, "etc/x"), "/etc/x");
        assert_eq!(remote_arg(MountScope::Full, ""), "/");
    }

    // ── shell quoting ────────────────────────────────────────────────────

    #[test]
    fn shell_squote_wraps_and_escapes_quotes() {
        assert_eq!(shell_squote("a b"), "'a b'");
        assert_eq!(shell_squote("it's"), "'it'\\''s'");
    }

    // ── the plan ─────────────────────────────────────────────────────────

    #[test]
    fn plan_builds_a_remote_to_remote_rsync() {
        let plan = plan_direct(
            "root",
            Path::new("/run/user/1000/mde-mesh/.mesh-ssh-key"),
            &RemoteEnd::new("oak", MountScope::Home, "docs/a.txt"),
            &RemoteEnd::new("birch", MountScope::Home, "incoming"),
            XferMode::Copy,
        );
        // C connects to the SOURCE peer; rsync runs there.
        assert_eq!(plan.ssh_target, "root@oak.mesh");
        // The remote command rsyncs the source straight to the dest peer.
        assert!(plan
            .remote_command
            .contains("rsync -a --stats --info=progress2"));
        assert!(plan.remote_command.contains("'docs/a.txt'"));
        assert!(plan.remote_command.contains("root@birch.mesh:incoming/"));
        // Uses the shared identity for both hops.
        assert!(plan
            .remote_command
            .contains("/run/user/1000/mde-mesh/.mesh-ssh-key"));
        // A copy never removes the source.
        assert!(!plan.remote_command.contains("--remove-source-files"));
        // The argv is a single ssh invocation to the source peer.
        assert_eq!(plan.argv[0], "ssh");
        assert_eq!(plan.argv.last().unwrap(), &plan.remote_command);
    }

    #[test]
    fn plan_move_removes_source_files() {
        let plan = plan_direct(
            "root",
            Path::new("/k"),
            &RemoteEnd::new("oak", MountScope::Home, "a.txt"),
            &RemoteEnd::new("birch", MountScope::Home, "dst"),
            XferMode::Move,
        );
        assert!(plan.remote_command.contains("--remove-source-files"));
    }

    #[test]
    fn plan_full_scope_uses_absolute_remote_paths() {
        let plan = plan_direct(
            "root",
            Path::new("/k"),
            &RemoteEnd::new("oak", MountScope::Full, "etc/hosts"),
            &RemoteEnd::new("birch", MountScope::Full, "tmp"),
            XferMode::Copy,
        );
        assert!(plan.remote_command.contains("'/etc/hosts'"));
        assert!(plan.remote_command.contains("root@birch.mesh:/tmp/"));
    }

    // ── rsync stats parsing ──────────────────────────────────────────────

    #[test]
    fn parse_rsync_stats_reads_files_and_bytes() {
        let stdout = "\
Number of files: 5 (reg: 3, dir: 2)
Number of regular files transferred: 3
Total file size: 12,345 bytes
Total transferred file size: 4,096 bytes
";
        let r = parse_rsync_stats(stdout);
        assert_eq!(r.files, 3);
        assert_eq!(r.bytes, 4_096);
    }

    #[test]
    fn parse_rsync_stats_defaults_to_zero() {
        assert_eq!(
            parse_rsync_stats("no stats here"),
            TransferReport::default()
        );
    }

    // ── the live backend is honestly gated (never fakes) ─────────────────

    #[test]
    fn live_backend_never_fakes_when_key_absent() {
        // With a plan pointing at a nonexistent key, the live backend MUST refuse
        // with a typed Gated (never Ok, never a fabricated transfer) — the §7 gate.
        let plan = plan_direct(
            "root",
            Path::new("/nonexistent/definitely/not/a/key"),
            &RemoteEnd::new("oak", MountScope::Home, "a"),
            &RemoteEnd::new("birch", MountScope::Home, "b"),
            XferMode::Copy,
        );
        let res = RsyncSshBackend::new().run(&plan);
        assert!(res.is_err(), "headless direct transfer must never succeed");
        assert!(matches!(res, Err(TransferError::Gated(_))));
    }

    // ── responder orchestration over fake seams (no ssh, no network) ─────

    struct FakeBackend {
        result: Mutex<Option<Result<TransferReport, TransferError>>>,
        seen: Mutex<Option<DirectPlan>>,
    }

    impl FakeBackend {
        fn new(result: Result<TransferReport, TransferError>) -> Arc<Self> {
            Arc::new(Self {
                result: Mutex::new(Some(result)),
                seen: Mutex::new(None),
            })
        }
    }

    impl TransferBackend for FakeBackend {
        fn run(&self, plan: &DirectPlan) -> Result<TransferReport, TransferError> {
            *self.seen.lock().unwrap() = Some(plan.clone());
            self.result
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| Ok(TransferReport::default()))
        }
    }

    struct FakeKeys;
    impl KeyProvider for FakeKeys {
        fn identity_key(&self) -> Result<PathBuf, MountError> {
            Ok(PathBuf::from("/tmp/fake-mesh-key"))
        }
    }

    fn responder(backend: Arc<dyn TransferBackend>) -> MeshTransfer {
        MeshTransfer::new(
            PathBuf::from("/run/user/1000"),
            PathBuf::from("/nonexistent-repo"),
            PathBuf::from("/nonexistent-wg"),
        )
        .with_backend(backend)
        .with_key_provider(Arc::new(FakeKeys))
    }

    fn req_json() -> String {
        json!({
            "op_id": 9,
            "src_host": "oak",
            "src_rel": "docs/a.txt",
            "dst_host": "birch",
            "dst_rel": "incoming",
            "mode": "copy",
        })
        .to_string()
    }

    #[test]
    fn direct_reply_ok_reports_files_and_bytes() {
        let backend = FakeBackend::new(Ok(TransferReport {
            files: 2,
            bytes: 2048,
        }));
        let r = responder(backend.clone());
        let reply = r.reply(DIRECT_VERB, Some(&req_json()));
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["files"], 2);
        assert_eq!(v["bytes"], 2048);
        assert_eq!(v["op_id"], 9);
        // The plan the backend saw is a real oak→birch rsync.
        let plan = backend.seen.lock().unwrap().clone().expect("ran");
        assert_eq!(plan.ssh_target, "root@oak.mesh");
        assert!(plan.remote_command.contains("root@birch.mesh:incoming/"));
    }

    #[test]
    fn direct_reply_gated_backend_marks_gated_for_relay() {
        let backend = FakeBackend::new(Err(TransferError::Gated("no ssh".into())));
        let reply = responder(backend).reply(DIRECT_VERB, Some(&req_json()));
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["gated"], true, "the surface relays a gated direct leg");
    }

    #[test]
    fn direct_reply_failed_backend_is_not_gated() {
        let backend = FakeBackend::new(Err(TransferError::Failed("rsync: denied".into())));
        let reply = responder(backend).reply(DIRECT_VERB, Some(&req_json()));
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["gated"], false, "a real rsync error surfaces, not relays");
    }

    #[test]
    fn direct_reply_rejects_same_host() {
        let body = json!({
            "src_host": "oak", "src_rel": "a", "dst_host": "oak", "dst_rel": "b",
        })
        .to_string();
        let backend = FakeBackend::new(Ok(TransferReport::default()));
        let reply = responder(backend.clone()).reply(DIRECT_VERB, Some(&body));
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("distinct peers"));
        // The backend never ran — a rejected request touches no ssh.
        assert!(backend.seen.lock().unwrap().is_none());
    }

    #[test]
    fn direct_reply_rejects_path_traversal() {
        let body = json!({
            "src_host": "oak", "src_rel": "../../etc/shadow",
            "dst_host": "birch", "dst_rel": "loot",
        })
        .to_string();
        let backend = FakeBackend::new(Ok(TransferReport::default()));
        let reply = responder(backend.clone()).reply(DIRECT_VERB, Some(&body));
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], false);
        assert!(
            backend.seen.lock().unwrap().is_none(),
            "unsafe path never runs"
        );
    }

    #[test]
    fn unknown_verb_is_a_typed_error() {
        let backend = FakeBackend::new(Ok(TransferReport::default()));
        let reply = responder(backend).reply("bogus", None);
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], false);
    }
}
