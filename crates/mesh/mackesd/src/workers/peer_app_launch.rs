//! WL-UX-005 — `peer_app_launch`: the peer-app remote-execution executor.
//!
//! The shell's unified Front Door lets an operator pick an app a *peer* node
//! advertises and launch it there. Front Door only PUBLISHES that intent — a
//! fire-and-forget `action/apps/launch` message carrying `{node, app_id, name,
//! source, mode}`
//! (`front_door::peer_app_launch_wire`). Before this worker nothing on the target
//! node consumed it, so the request was inert. This worker is the missing
//! consumer: it runs on **every workstation node**, drains `action/apps/launch`,
//! and — only for requests addressed to *its own* node id — actually launches the
//! requested app locally.
//!
//! ## Security (load-bearing)
//!
//! The mesh Bus is already peer-authenticated, but a launch is remote code
//! execution, so this worker adds a hard allowlist on top:
//!
//! - It NEVER execs an arbitrary command string from the wire. The wire carries
//!   only an opaque `app_id` (a `.desktop` file id), never an `exec` line.
//! - It resolves that `app_id` against **this node's own advertised app catalog**
//!   — the exact same [`crate::ipc::apps::scan_local_apps`] scan the node
//!   publishes to the peer-app catalog (`action/apps/list` / `peer-list`). An id
//!   that is not in the node's own published list is refused, no exec.
//!   The argv that actually runs comes from the RESOLVED catalog entry's `Exec`
//!   line ([`launch_argv`], field-codes stripped), never from the request.
//! - A malformed request (non-JSON, missing `node`/`app_id`, or one addressed to
//!   another node) is refused with no side effect.
//!
//! Every accepted launch and every refusal is logged (the requested node + id and
//! the resolved binary) so a remote launch is always auditable.
//!
//! ## Shape (mirrors [`crate::workers::container`])
//!
//! A per-node bus-drain worker: an [`AppLauncher`] trait is the sole seam to the
//! outside (production [`SpawnLauncher`] does a real detached `Command::spawn`;
//! tests inject a recording fake), so the resolve → refuse/launch decision is
//! fully unit-tested with no real process spawn. The cursor is primed to the tail
//! on start so a restart never re-launches the backlog.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};
use crate::ipc::apps::{default_app_dirs, scan_local_apps, AppEntry};
use mackes_mesh_types::vdi_session::{AppVmLaunchRequest, SessionRequest};
use mackes_mesh_types::workloads::WorkloadId;

use super::{ShutdownToken, Worker};

/// The flat Bus topic this worker drains. Per-node targeting is via the request's
/// `node` field, not the topic (the same shape [`crate::workers::container`] uses).
pub const ACTION_TOPIC: &str = "action/apps/launch";

/// Action-drain cadence. The bus read is a cheap local log scan; a launch is a
/// rare, operator-initiated event, so a 1 s poll is responsive without spinning.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// A bounded page prevents an attacker with Bus write access from making one
/// poll allocate or execute an unbounded action backlog. The cursor commits
/// only after the complete page has been staged and the Bus identity rechecked.
const MAX_ACTIONS_PER_TICK: usize = 32;
const MAX_ACTION_BODY_BYTES: usize = 64 * 1024;
const MAX_JOURNAL_BYTES: usize = 4 * 1024 * 1024;
const MAX_JOURNAL_RECORDS: usize = 2_048;
const JOURNAL_SCHEMA_VERSION: u16 = 1;
const RESULT_SCHEMA_VERSION: u16 = 1;
const JOURNAL_FILE: &str = "peer-app-launch-journal.json";

/// Shared-Bus capability context for the remote app-launch mutation. The
/// worker binds the capability to this node (the only node whose catalog it
/// may resolve) and the requested catalog id.
const PEER_APP_LAUNCH_AUTH_VERB: &str = "peer-app-launch";

/// Exact-request result lane. A durable terminal record is republished when a
/// new Bus generation appears, giving callers corrected-forward truth without
/// repeating the launch effect.
#[must_use]
pub fn launch_result_topic(request_ulid: &str) -> String {
    format!("reply/{request_ulid}")
}

/// Stable audit reason for refusing a guest-owned Flatpak at this legacy host
/// launcher. Keep this machine-readable so operators can distinguish policy
/// refusal from a missing catalog entry or a failed process spawn.
pub const FLATPAK_LEGACY_LAUNCH_REFUSAL_REASON: &str =
    "guest-owned-flatpak-cannot-use-legacy-host-launcher";

/// The catalog provenance carried on the launch wire. `Flatpak` is guest-owned
/// and must never be handed to this worker's host `.desktop` launcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchSource {
    /// A normal host/XDG `.desktop` application.
    Xdg,
    /// A guest-owned Flatpak application.
    Flatpak,
}

impl LaunchSource {
    fn parse(raw: Option<&str>) -> Option<Self> {
        match raw?.trim().to_ascii_lowercase().as_str() {
            "xdg" => Some(Self::Xdg),
            "flatpak" => Some(Self::Flatpak),
            _ => None,
        }
    }

    const fn wire(self) -> &'static str {
        match self {
            Self::Xdg => "xdg",
            Self::Flatpak => "flatpak",
        }
    }
}

/// The execution plane requested by Front Door. This worker implements only
/// the legacy host plane; guest App VM execution belongs to a later worker and
/// must not silently fall through here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    /// Execute a validated host `.desktop` entry through this worker.
    LegacyHost,
    /// Route a guest-owned app through an App VM launcher (not implemented here).
    GuestAppVm,
}

impl LaunchMode {
    fn parse(raw: Option<&str>) -> Option<Self> {
        match raw?.trim().to_ascii_lowercase().as_str() {
            "legacy-host" => Some(Self::LegacyHost),
            "guest-app-vm" => Some(Self::GuestAppVm),
            _ => None,
        }
    }

    const fn wire(self) -> &'static str {
        match self {
            Self::LegacyHost => "legacy-host",
            Self::GuestAppVm => "guest-app-vm",
        }
    }
}

// ───────────────────────────── request model ─────────────────────────────

/// A parsed `action/apps/launch` request. `app_id` is an opaque catalog id,
/// NEVER a command line. `source` and `mode` are policy fields, not hints: they
/// determine which execution plane may receive the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRequest {
    /// The target node this launch is addressed to.
    pub node: String,
    /// The `.desktop` catalog id to resolve against the node's own app list.
    pub app_id: String,
    /// The display name (advisory — for logs only; never used to resolve).
    pub name: String,
    /// The catalog provenance asserted by the publisher.
    pub source: LaunchSource,
    /// The execution plane asserted by the publisher.
    pub mode: LaunchMode,
    /// Stable session identity required for a guest App VM launch.
    pub session_id: Option<String>,
    /// Explicit admitted App VM identity; placement is never guessed here.
    pub vm_id: Option<String>,
    /// Signed catalog revision required for guest resolution.
    pub catalog_revision: Option<String>,
    /// Named guest profile, never an image path or command.
    pub guest_profile: Option<String>,
    /// Capabilities requested by the catalog/policy.
    pub requested_capabilities: Vec<String>,
    /// Whether the guest session should resume when available.
    pub resume: bool,
    /// Shell peer that will drive the application surface.
    pub client_peer: Option<String>,
}

impl LaunchRequest {
    /// Whether this request is addressed to `node_id`. An empty target never
    /// matches — the worker refuses to guess which node a launch is for.
    #[must_use]
    pub fn targets(&self, node_id: &str) -> bool {
        !self.node.is_empty() && self.node == node_id
    }
}

/// Parse one `action/apps/launch` body. `None` for non-JSON, an unknown explicit
/// source/mode, or a request missing a `node` or `app_id` — a malformed request
/// is refused, never guessed. Missing source/mode remain temporarily compatible
/// with older normal catalog publishers as `xdg` + `legacy-host`; catalog-source
/// matching below still prevents a Flatpak from using that compatibility path.
#[must_use]
pub fn parse_launch_request(body: &str) -> Option<LaunchRequest> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let node = v.get("node").and_then(|n| n.as_str()).unwrap_or("").trim();
    let app_id = v
        .get("app_id")
        .and_then(|a| a.as_str())
        .unwrap_or("")
        .trim();
    if node.is_empty() || app_id.is_empty() {
        return None;
    }
    let name = v
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .trim()
        .to_owned();
    let source = v
        .get("source")
        .and_then(serde_json::Value::as_str)
        .map_or(Some(LaunchSource::Xdg), |raw| {
            LaunchSource::parse(Some(raw))
        })?;
    let mode = v
        .get("mode")
        .and_then(serde_json::Value::as_str)
        .map_or(Some(LaunchMode::LegacyHost), |raw| {
            LaunchMode::parse(Some(raw))
        })?;
    Some(LaunchRequest {
        node: node.to_owned(),
        app_id: app_id.to_owned(),
        name,
        source,
        mode,
        session_id: v
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        vm_id: v
            .get("vm_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        catalog_revision: v
            .get("catalog_revision")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        guest_profile: v
            .get("guest_profile")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        requested_capabilities: v
            .get("requested_capabilities")
            .and_then(serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        resume: v
            .get("resume")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        client_peer: v
            .get("client_peer")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
    })
}

/// Convert an authorized guest launch envelope into the existing VDI session
/// verb. No placement is selected and no Bus write occurs here; callers must
/// publish the returned request through the normal signed session-action path.
#[must_use]
pub fn app_vm_session_request(req: &LaunchRequest) -> Option<SessionRequest> {
    if req.source != LaunchSource::Flatpak || req.mode != LaunchMode::GuestAppVm {
        return None;
    }
    let session_id = req.session_id.clone()?;
    let vm_id = req.vm_id.clone()?;
    let catalog_revision = req.catalog_revision.clone()?;
    let guest_profile = req.guest_profile.clone()?;
    let client_peer = req.client_peer.clone()?;
    // These route identities cross from the peer-app envelope into the
    // Workloads/session authority. Reject values that would be interpreted
    // differently by a later filesystem, libvirt, or replicated-session
    // boundary instead of publishing a lifecycle mutation that can never be
    // admitted. The App VM declaration below separately validates the session,
    // catalog, profile, application, and capability identities.
    WorkloadId::new(req.node.clone()).ok()?;
    WorkloadId::new(vm_id.clone()).ok()?;
    WorkloadId::new(client_peer.clone()).ok()?;
    let launch = AppVmLaunchRequest::new(
        req.app_id.clone(),
        catalog_revision.clone(),
        guest_profile.clone(),
        req.requested_capabilities.clone(),
        session_id.clone(),
        req.resume,
    )
    .ok()?;
    // The wire constructor only checks shape.  This boundary is where an
    // authorized launch becomes a lifecycle mutation, so enforce the stronger
    // App-VM admission policy before projecting it onto the session bus.
    launch.validate_admitted().ok()?;
    Some(SessionRequest::OpenApp {
        id: session_id,
        serving_peer: req.node.clone(),
        vm_id,
        client_peer,
        app_id: req.app_id.clone(),
        catalog_revision,
        guest_profile,
        requested_capabilities: req.requested_capabilities.clone(),
        resume: req.resume,
    })
}

/// Build the launch argv from a resolved catalog entry's `.desktop` `Exec` line.
/// Field codes (`%U`, `%F`, …) are dropped and any `env VAR=val` prefix is
/// stripped, so the binary is exec'd directly (the same token filter
/// [`crate::ipc::apps::exec_binary`] already uses to find the binary, extended to
/// keep the trailing real arguments). `None` for an empty / field-code-only line.
#[must_use]
pub fn launch_argv(exec: &str) -> Option<Vec<String>> {
    // Drop desktop-entry field codes first — they are placeholders the launcher
    // fills, never something we forward.
    let tokens: Vec<String> = exec
        .split_whitespace()
        .filter(|t| !t.starts_with('%'))
        .map(str::to_string)
        .collect();
    // The binary is the first token that is neither the `env` shim nor a
    // `KEY=VALUE` environment assignment; everything from there on is the argv.
    let start = tokens.iter().position(|t| t != "env" && !t.contains('='))?;
    let argv: Vec<String> = tokens[start..].to_vec();
    if argv.is_empty() {
        None
    } else {
        Some(argv)
    }
}

// ───────────────────────────── launcher seam ─────────────────────────────

/// The sole outside effect of the worker — launching a resolved app. Injectable so
/// the resolve/allowlist logic is unit-tested without spawning real processes.
pub trait AppLauncher {
    /// Launch `argv` (already resolved from the node's own catalog, field codes
    /// stripped). `argv[0]` is the binary. Returns an error the caller logs; it
    /// must never block on the child (a launched GUI app outlives this call).
    fn launch(&self, argv: &[String]) -> std::io::Result<()>;
}

/// Production launcher: a real detached `Command::spawn` with a null stdio so the
/// launched app never inherits the daemon's descriptors and the daemon never waits
/// on it.
#[derive(Debug, Default)]
pub struct SpawnLauncher;

impl AppLauncher for SpawnLauncher {
    fn launch(&self, argv: &[String]) -> std::io::Result<()> {
        let (binary, args) = argv
            .split_first()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty argv"))?;
        Command::new(binary)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_child| ())
    }
}

// ───────────────────────────── bus plumbing ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct BusIdentity {
    device: u64,
    inode: u64,
}

struct BusTransaction {
    persist: Persist,
    root: PathBuf,
    identity: BusIdentity,
}

impl BusTransaction {
    fn open(root: &Path) -> io::Result<Self> {
        // Persist creates an absent index. Drop that initializer, then bind a
        // fresh connection between two path-identity observations.
        if !root.join("index.sqlite").exists() {
            drop(Persist::open(root.to_path_buf()).map_err(io_other)?);
        }
        let before = bus_identity(root)?;
        let persist = Persist::open(root.to_path_buf()).map_err(io_other)?;
        let after = bus_identity(root)?;
        if before != after {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "peer-app launch Bus changed while opening",
            ));
        }
        Ok(Self {
            persist,
            root: root.to_path_buf(),
            identity: after,
        })
    }

    fn verify_current(&self) -> io::Result<()> {
        if bus_identity(&self.root)? != self.identity {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "peer-app launch Bus connection names a retired index",
            ));
        }
        Ok(())
    }
}

fn bus_identity(root: &Path) -> io::Result<BusIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = fs::metadata(root.join("index.sqlite"))?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "peer-app launch Bus index is not a regular file",
        ));
    }
    Ok(BusIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum LaunchResultStatus {
    Succeeded,
    Refused,
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct LaunchResult {
    schema_version: u16,
    request_ulid: String,
    node: String,
    app_id: Option<String>,
    status: LaunchResultStatus,
    reason: String,
}

impl LaunchResult {
    fn new(
        request_ulid: &str,
        node: &str,
        app_id: Option<String>,
        status: LaunchResultStatus,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: RESULT_SCHEMA_VERSION,
            request_ulid: request_ulid.to_owned(),
            node: node.to_owned(),
            app_id,
            status,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
enum LaunchJournalPhase {
    Prepared,
    EffectClaimed,
    Terminal {
        result: LaunchResult,
        delivered_to: Option<BusIdentity>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct LaunchJournalRecord {
    body: String,
    node: String,
    app_id: String,
    argv: Vec<String>,
    phase: LaunchJournalPhase,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct LaunchJournalState {
    schema_version: u16,
    records: BTreeMap<String, LaunchJournalRecord>,
}

impl Default for LaunchJournalState {
    fn default() -> Self {
        Self {
            schema_version: JOURNAL_SCHEMA_VERSION,
            records: BTreeMap::new(),
        }
    }
}

struct LaunchJournal {
    root: PathBuf,
    path: PathBuf,
    state: LaunchJournalState,
}

impl LaunchJournal {
    fn open(root: &Path) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        require_safe_directory(root)?;
        let path = root.join(JOURNAL_FILE);
        let state = match read_bounded_regular_file(&path, MAX_JOURNAL_BYTES) {
            Ok(bytes) => serde_json::from_slice::<LaunchJournalState>(&bytes).map_err(io_other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => LaunchJournalState::default(),
            Err(error) => return Err(error),
        };
        if state.schema_version != JOURNAL_SCHEMA_VERSION
            || state.records.len() > MAX_JOURNAL_RECORDS
            || state.records.iter().any(|(ulid, record)| {
                ulid.is_empty()
                    || ulid.len() > 64
                    || record.body.len() > MAX_ACTION_BODY_BYTES
                    || record.argv.len() > 64
                    || record.argv.iter().any(|part| part.len() > 4_096)
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "peer-app launch journal is invalid or exceeds its bounds",
            ));
        }
        let mut journal = Self {
            root: root.to_path_buf(),
            path,
            state,
        };
        journal.recover_ambiguous_claims()?;
        Ok(journal)
    }

    fn recover_ambiguous_claims(&mut self) -> io::Result<()> {
        let mut changed = false;
        for (ulid, record) in &mut self.state.records {
            let reason = match &record.phase {
                LaunchJournalPhase::Prepared => Some(
                    "authorization interrupted after durable preparation; launch was not retried",
                ),
                LaunchJournalPhase::EffectClaimed => Some(
                    "launch outcome is indeterminate after recovery of its durable effect claim; launch was not repeated",
                ),
                LaunchJournalPhase::Terminal { .. } => None,
            };
            if let Some(reason) = reason {
                record.phase = LaunchJournalPhase::Terminal {
                    result: LaunchResult::new(
                        ulid,
                        &record.node,
                        Some(record.app_id.clone()),
                        LaunchResultStatus::Indeterminate,
                        reason,
                    ),
                    delivered_to: None,
                };
                changed = true;
            }
        }
        if changed {
            self.store()?;
        }
        Ok(())
    }

    fn make_room(&mut self) -> io::Result<()> {
        if self.state.records.len() < MAX_JOURNAL_RECORDS {
            return Ok(());
        }
        let removable = self
            .state
            .records
            .iter()
            .find_map(|(ulid, record)| match &record.phase {
                LaunchJournalPhase::Terminal {
                    delivered_to: Some(_),
                    ..
                } => Some(ulid.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                io::Error::other("peer-app launch journal is full of undelivered records")
            })?;
        self.state.records.remove(&removable);
        Ok(())
    }

    fn insert_prepared(
        &mut self,
        ulid: &str,
        body: String,
        node: String,
        app_id: String,
        argv: Vec<String>,
    ) -> io::Result<()> {
        if self.state.records.contains_key(ulid) {
            return Ok(());
        }
        self.make_room()?;
        self.state.records.insert(
            ulid.to_owned(),
            LaunchJournalRecord {
                body,
                node,
                app_id,
                argv,
                phase: LaunchJournalPhase::Prepared,
            },
        );
        self.store()
    }

    fn insert_terminal(&mut self, result: LaunchResult) -> io::Result<()> {
        if self.state.records.contains_key(&result.request_ulid) {
            return Ok(());
        }
        self.make_room()?;
        self.state.records.insert(
            result.request_ulid.clone(),
            LaunchJournalRecord {
                body: String::new(),
                node: result.node.clone(),
                app_id: result.app_id.clone().unwrap_or_default(),
                argv: Vec::new(),
                phase: LaunchJournalPhase::Terminal {
                    result,
                    delivered_to: None,
                },
            },
        );
        self.store()
    }

    fn set_phase(&mut self, ulid: &str, phase: LaunchJournalPhase) -> io::Result<()> {
        self.state
            .records
            .get_mut(ulid)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "launch record disappeared"))?
            .phase = phase;
        self.store()
    }

    fn store(&self) -> io::Result<()> {
        require_safe_directory(&self.root)?;
        let body = serde_json::to_vec(&self.state).map_err(io_other)?;
        if body.len() > MAX_JOURNAL_BYTES {
            return Err(io::Error::other(
                "peer-app launch journal exceeds its byte bound",
            ));
        }
        atomic_write_file(&self.path, &body)
    }
}

fn require_safe_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "peer-app launch state root is not a safe directory",
        ));
    }
    Ok(())
}

fn read_bounded_regular_file(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    let file: File = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?
    .into();
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > max_bytes as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "peer-app launch state file is not a bounded regular file",
        ));
    }
    let mut body = Vec::with_capacity((metadata.len() as usize).saturating_add(1));
    file.take((max_bytes as u64).saturating_add(1))
        .read_to_end(&mut body)?;
    if body.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "peer-app launch state file exceeds its bound",
        ));
    }
    Ok(body)
}

fn atomic_write_file(path: &Path, body: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "state path has no parent"))?;
    require_safe_directory(parent)?;
    let temporary = parent.join(format!(".{JOURNAL_FILE}.tmp"));
    let mut file: File = rustix::fs::open(
        &temporary,
        rustix::fs::OFlags::WRONLY
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::TRUNC
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )?
    .into();
    file.write_all(body)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    File::open(parent)?.sync_all()
}

fn default_bus_root() -> Option<PathBuf> {
    mde_bus::default_data_dir()
}

fn default_home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from)
}

fn default_state_root() -> PathBuf {
    crate::default_db_path()
        .parent()
        .map(|parent| parent.join("peer-app-launch"))
        .unwrap_or_else(|| PathBuf::from("/var/lib/mde/peer-app-launch"))
}

// ───────────────────────────── the worker ─────────────────────────────

/// The WL-UX-005 peer-app remote-execution executor.
pub struct PeerAppLaunchWorker {
    /// This node's id — the ONLY `node` value a request may target for this worker
    /// to act (`LaunchRequest::targets`).
    node_id: String,
    /// Home dir whose XDG app dirs are scanned to build the allowlist. Overridable
    /// in tests so a fixture catalog can stand in for the real one.
    home: PathBuf,
    /// The injectable launch seam (production: [`SpawnLauncher`]).
    launcher: Arc<dyn AppLauncher + Send + Sync>,
    /// Action-drain cadence.
    poll: Duration,
    /// Bus root override (tests). `None` ⇒ [`default_bus_root`].
    bus_root_override: Option<PathBuf>,
    /// Host-local state, deliberately outside the replaceable Bus generation.
    state_root: PathBuf,
    /// Shared, fail-closed authorization gate for the remote launch mutation.
    authorizer: Arc<ActionAuthorizer>,
    #[cfg(test)]
    after_action_read: Option<Arc<dyn Fn(&Path) + Send + Sync>>,
    #[cfg(test)]
    after_result_write: Option<Arc<dyn Fn(&Path) + Send + Sync>>,
}

impl PeerAppLaunchWorker {
    /// Construct with production defaults: the live [`SpawnLauncher`], the real
    /// `HOME` app dirs, the default cadence, and the auto-resolved bus root.
    /// `node_id` is the sole launch target this worker acts on.
    #[must_use]
    pub fn new(node_id: String) -> Self {
        Self {
            node_id,
            home: default_home(),
            launcher: Arc::new(SpawnLauncher),
            poll: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
            state_root: default_state_root(),
            authorizer: Arc::new(ActionAuthorizer::production()),
            #[cfg(test)]
            after_action_read: None,
            #[cfg(test)]
            after_result_write: None,
        }
    }

    /// Inject the launch seam (tests). Production uses the [`SpawnLauncher`] default.
    #[must_use]
    pub fn with_launcher(mut self, launcher: Arc<dyn AppLauncher + Send + Sync>) -> Self {
        self.launcher = launcher;
        self
    }

    /// Override the app-catalog home (tests).
    #[must_use]
    pub fn with_home(mut self, home: PathBuf) -> Self {
        self.home = home;
        self
    }

    /// Override the action-drain cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Override the Bus root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    /// Override the host-local journal root (tests).
    #[cfg(test)]
    #[must_use]
    fn with_state_root(mut self, root: PathBuf) -> Self {
        self.state_root = root;
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_after_action_read(mut self, hook: impl Fn(&Path) + Send + Sync + 'static) -> Self {
        self.after_action_read = Some(Arc::new(hook));
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_after_result_write(mut self, hook: impl Fn(&Path) + Send + Sync + 'static) -> Self {
        self.after_result_write = Some(Arc::new(hook));
        self
    }

    /// Inject an isolated verifier and replay ledger for hostile action tests.
    /// Production always uses the systemd-credential-backed authorizer.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    fn bus_root(&self) -> Option<PathBuf> {
        self.bus_root_override.clone().or_else(default_bus_root)
    }

    /// Resolve `app_id` against THIS node's own advertised app catalog. The
    /// allowlist: only an id present in [`scan_local_apps`] (the same list the node
    /// publishes to peers) resolves; anything else is `None` (refused).
    fn resolve_advertised_app(&self, app_id: &str) -> Option<AppEntry> {
        let app_id = app_id.trim();
        if app_id.is_empty() {
            return None;
        }
        scan_local_apps(&default_app_dirs(&self.home))
            .into_iter()
            .find(|entry| entry.id == app_id)
    }

    fn resolve_launch_argv(&self, req: &LaunchRequest) -> Result<Vec<String>, &'static str> {
        let Some(app) = self.resolve_advertised_app(&req.app_id) else {
            return Err("app-is-not-in-local-advertised-catalog");
        };
        if !self.legacy_launch_allowed(req, &app) {
            return Err("launch-source-or-mode-refused");
        }
        launch_argv(&app.exec).ok_or("advertised-app-has-no-runnable-exec")
    }

    /// Enforce the legacy launcher's source/mode boundary before it can reach
    /// [`AppLauncher`]. Flatpak is guest-owned even when a matching exported
    /// `.desktop` file is visible on the host, so it is never resolved into a
    /// host process here.
    fn legacy_launch_allowed(&self, req: &LaunchRequest, app: &AppEntry) -> bool {
        if req.source == LaunchSource::Flatpak {
            tracing::warn!(
                target: "mackesd::peer_app_launch",
                node = %req.node,
                app_id = %req.app_id,
                source = req.source.wire(),
                mode = req.mode.wire(),
                reason = FLATPAK_LEGACY_LAUNCH_REFUSAL_REASON,
                "peer_app_launch: REFUSED — {FLATPAK_LEGACY_LAUNCH_REFUSAL_REASON}",
            );
            return false;
        }
        if req.mode != LaunchMode::LegacyHost {
            tracing::warn!(
                target: "mackesd::peer_app_launch",
                node = %req.node,
                app_id = %req.app_id,
                source = req.source.wire(),
                mode = req.mode.wire(),
                reason = "unsupported-launch-mode-on-legacy-host",
                "peer_app_launch: REFUSED — launch mode is not supported by the legacy host launcher",
            );
            return false;
        }
        if app.source.trim().eq_ignore_ascii_case("flatpak") {
            tracing::warn!(
                target: "mackesd::peer_app_launch",
                node = %req.node,
                app_id = %req.app_id,
                request_source = req.source.wire(),
                catalog_source = %app.source,
                mode = req.mode.wire(),
                reason = FLATPAK_LEGACY_LAUNCH_REFUSAL_REASON,
                "peer_app_launch: REFUSED — catalog source is guest-owned Flatpak, not a host app",
            );
            return false;
        }
        if !app.source.trim().eq_ignore_ascii_case(req.source.wire()) {
            tracing::warn!(
                target: "mackesd::peer_app_launch",
                node = %req.node,
                app_id = %req.app_id,
                request_source = req.source.wire(),
                catalog_source = %app.source,
                mode = req.mode.wire(),
                reason = "launch-source-does-not-match-catalog",
                "peer_app_launch: REFUSED — request source does not match the advertised catalog entry",
            );
            return false;
        }
        true
    }

    /// Handle one parsed request: enforce the node-target gate and the catalog
    /// allowlist, then launch. Returns `true` iff an app was actually launched.
    /// Pure over the injected launcher, so it is fully unit-tested.
    #[cfg(test)]
    fn handle_request(&self, req: &LaunchRequest) -> bool {
        if !req.targets(&self.node_id) {
            // Not addressed to this node — silently advance (another node's worker
            // owns it). Not logged per-request to avoid fan-out log spam.
            return false;
        }
        let Some(app) = self.resolve_advertised_app(&req.app_id) else {
            tracing::warn!(
                target: "mackesd::peer_app_launch",
                node = %req.node,
                app_id = %req.app_id,
                "peer_app_launch: REFUSED — '{}' is not in this node's advertised app catalog",
                req.app_id,
            );
            return false;
        };
        if !self.legacy_launch_allowed(req, &app) {
            return false;
        }
        let Some(argv) = launch_argv(&app.exec) else {
            tracing::warn!(
                target: "mackesd::peer_app_launch",
                node = %req.node,
                app_id = %req.app_id,
                exec = %app.exec,
                "peer_app_launch: REFUSED — advertised app has no runnable Exec line",
            );
            return false;
        };
        match self.launcher.launch(&argv) {
            Ok(()) => {
                tracing::info!(
                    target: "mackesd::peer_app_launch",
                    node = %req.node,
                    app_id = %app.id,
                    app_name = %app.name,
                    binary = %argv[0],
                    "peer_app_launch: launched advertised app on this node (peer-requested)",
                );
                true
            }
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::alert",
                    node = %req.node,
                    app_id = %app.id,
                    binary = %argv[0],
                    "ALERT (warn): peer_app_launch failed to spawn '{}' — {e}",
                    argv[0],
                );
                false
            }
        }
    }

    /// Authenticate and handle one raw Bus body. Authorization intentionally
    /// precedes catalog resolution and the launcher seam: a shared-spool
    /// publisher cannot turn a transport write into a process spawn.
    #[cfg(test)]
    fn handle_body(&self, body: &str) -> bool {
        let Some(req) = parse_launch_request(body) else {
            tracing::warn!(
                target: "mackesd::peer_app_launch",
                "peer_app_launch: malformed launch request refused (no node/app_id)",
            );
            return false;
        };
        if let Err(error) = self.authorizer.authorize(
            body,
            MutationContext {
                verb: PEER_APP_LAUNCH_AUTH_VERB,
                node: &self.node_id,
                target: &req.app_id,
            },
        ) {
            tracing::warn!(
                target: "mackesd::peer_app_launch",
                node = %self.node_id,
                app_id = %req.app_id,
                %error,
                "peer_app_launch: refused unauthorized launch request",
            );
            return false;
        }
        self.handle_request(&req)
    }

    fn publish_pending_results(
        &self,
        transaction: &BusTransaction,
        journal: &mut LaunchJournal,
    ) -> io::Result<bool> {
        let pending = journal
            .state
            .records
            .iter()
            .filter_map(|(ulid, record)| match &record.phase {
                LaunchJournalPhase::Terminal {
                    result,
                    delivered_to,
                } if *delivered_to != Some(transaction.identity) => {
                    Some((ulid.clone(), result.clone()))
                }
                _ => None,
            })
            .take(MAX_ACTIONS_PER_TICK)
            .collect::<Vec<_>>();
        for (ulid, result) in &pending {
            let body = serde_json::to_string(result).map_err(io_other)?;
            let topic = launch_result_topic(ulid);
            transaction.verify_current()?;
            let already_present = transaction
                .persist
                .read_latest(&topic)
                .map_err(io_other)?
                .and_then(|message| message.body)
                .is_some_and(|existing| existing == body);
            transaction.verify_current()?;
            if !already_present {
                transaction
                    .persist
                    .write(&topic, Priority::Default, None, Some(&body))
                    .map_err(io_other)?;
                #[cfg(test)]
                if let Some(hook) = &self.after_result_write {
                    hook(&transaction.root);
                }
            }
            transaction.verify_current()?;
            let record = journal.state.records.get_mut(ulid).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "launch result record disappeared")
            })?;
            record.phase = LaunchJournalPhase::Terminal {
                result: result.clone(),
                delivered_to: Some(transaction.identity),
            };
            journal.store()?;
            transaction.verify_current()?;
        }
        Ok(!journal.state.records.values().any(|record| {
            matches!(
                &record.phase,
                LaunchJournalPhase::Terminal { delivered_to, .. }
                    if *delivered_to != Some(transaction.identity)
            )
        }))
    }

    fn activate_bus(
        &self,
        transaction: &BusTransaction,
        journal: &mut LaunchJournal,
    ) -> io::Result<Option<Option<String>>> {
        // Snapshot exactly the retained tail at activation. Actions written
        // after this read are forward work and remain visible via list_since.
        let tail = transaction
            .persist
            .read_latest(ACTION_TOPIC)
            .map_err(io_other)?
            .map(|message| message.ulid);
        transaction.verify_current()?;
        if !self.publish_pending_results(transaction, journal)? {
            return Ok(None);
        }
        transaction.verify_current()?;
        Ok(Some(tail))
    }

    fn terminal_for(
        &self,
        ulid: &str,
        app_id: Option<String>,
        status: LaunchResultStatus,
        reason: impl Into<String>,
    ) -> LaunchResult {
        LaunchResult::new(ulid, &self.node_id, app_id, status, reason)
    }

    fn process_action(
        &self,
        transaction: &BusTransaction,
        journal: &mut LaunchJournal,
        ulid: &str,
        body: String,
    ) -> io::Result<()> {
        if let Some(existing) = journal.state.records.get(ulid) {
            if matches!(&existing.phase, LaunchJournalPhase::Terminal { .. }) {
                return Ok(());
            }
            let app_id = Some(existing.app_id.clone());
            journal.set_phase(
                ulid,
                LaunchJournalPhase::Terminal {
                    result: self.terminal_for(
                        ulid,
                        app_id,
                        LaunchResultStatus::Indeterminate,
                        "recovered non-terminal launch record; effect was not repeated",
                    ),
                    delivered_to: None,
                },
            )?;
            return Ok(());
        }
        if body.len() > MAX_ACTION_BODY_BYTES {
            return journal.insert_terminal(self.terminal_for(
                ulid,
                None,
                LaunchResultStatus::Refused,
                "action-body-exceeds-bound",
            ));
        }
        let Some(request) = parse_launch_request(&body) else {
            return journal.insert_terminal(self.terminal_for(
                ulid,
                None,
                LaunchResultStatus::Refused,
                "malformed-launch-request",
            ));
        };
        if !request.targets(&self.node_id) {
            return Ok(());
        }
        let argv = match self.resolve_launch_argv(&request) {
            Ok(argv) => argv,
            Err(reason) => {
                return journal.insert_terminal(self.terminal_for(
                    ulid,
                    Some(request.app_id),
                    LaunchResultStatus::Refused,
                    reason,
                ));
            }
        };
        // Preparation durably binds the exact authorized body and resolved
        // argv before the capability nonce or launch effect can be consumed.
        journal.insert_prepared(
            ulid,
            body.clone(),
            self.node_id.clone(),
            request.app_id.clone(),
            argv.clone(),
        )?;
        transaction.verify_current()?;
        if let Err(error) = self.authorizer.authorize(
            &body,
            MutationContext {
                verb: PEER_APP_LAUNCH_AUTH_VERB,
                node: &self.node_id,
                target: &request.app_id,
            },
        ) {
            return journal.set_phase(
                ulid,
                LaunchJournalPhase::Terminal {
                    result: self.terminal_for(
                        ulid,
                        Some(request.app_id),
                        LaunchResultStatus::Refused,
                        format!("authorization-refused: {error}"),
                    ),
                    delivered_to: None,
                },
            );
        }
        journal.set_phase(ulid, LaunchJournalPhase::EffectClaimed)?;
        // A replacement after the body read but before this point retires the
        // transaction and converts the durable claim to indeterminate on the
        // next pass. It is never allowed to launch from the retired generation.
        transaction.verify_current()?;
        let (status, reason) = match self.launcher.launch(&argv) {
            Ok(()) => (LaunchResultStatus::Succeeded, "launch-spawned".to_string()),
            Err(error) => (
                LaunchResultStatus::Indeterminate,
                format!("launch returned after durable effect claim: {error}"),
            ),
        };
        journal.set_phase(
            ulid,
            LaunchJournalPhase::Terminal {
                result: self.terminal_for(ulid, Some(request.app_id), status, reason),
                delivered_to: None,
            },
        )
    }

    fn process_page(
        &self,
        transaction: &BusTransaction,
        journal: &mut LaunchJournal,
        cursor: &mut Option<String>,
    ) -> io::Result<()> {
        let messages = transaction
            .persist
            .list_since_limit(ACTION_TOPIC, cursor.as_deref(), MAX_ACTIONS_PER_TICK)
            .map_err(io_other)?;
        // `list_since_limit` has fully materialized this bounded page. No
        // cursor or effect changes occur until its connection still names the
        // path's current index.
        #[cfg(test)]
        if let Some(hook) = &self.after_action_read {
            hook(&transaction.root);
        }
        transaction.verify_current()?;
        let page_tail = messages.last().map(|message| message.ulid.clone());
        for message in messages {
            self.process_action(
                transaction,
                journal,
                &message.ulid,
                message.body.unwrap_or_default(),
            )?;
        }
        self.publish_pending_results(transaction, journal)?;
        transaction.verify_current()?;
        if let Some(tail) = page_tail {
            *cursor = Some(tail);
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Worker for PeerAppLaunchWorker {
    fn name(&self) -> &'static str {
        "peer_app_launch"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut journal = LaunchJournal::open(&self.state_root)
            .map_err(|error| anyhow::anyhow!("open peer-app launch journal: {error}"))?;
        let mut active_identity = None;
        let mut cursor = None;
        loop {
            if let Some(root) = self.bus_root() {
                match BusTransaction::open(&root) {
                    Ok(transaction) => {
                        if active_identity != Some(transaction.identity) {
                            match self.activate_bus(&transaction, &mut journal) {
                                Ok(Some(tail)) => {
                                    cursor = tail;
                                    active_identity = Some(transaction.identity);
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    tracing::warn!(target: "mackesd::peer_app_launch", %error, "peer-app launch Bus activation deferred")
                                }
                            }
                        } else if let Err(error) =
                            self.process_page(&transaction, &mut journal, &mut cursor)
                        {
                            tracing::warn!(target: "mackesd::peer_app_launch", %error, "peer-app launch Bus transaction deferred");
                            active_identity = None;
                        }
                    }
                    Err(error) => {
                        tracing::warn!(target: "mackesd::peer_app_launch", %error, "peer-app launch Bus unavailable")
                    }
                }
            }
            tokio::select! {
                () = shutdown.wait() => break,
                () = tokio::time::sleep(self.poll) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    const AUTH_KEY: &[u8] = b"peer-app-launch-action-auth-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    /// A recording launcher: never spawns a real process, just captures the argv
    /// each launch would have run so the resolve/allowlist decision is asserted.
    #[derive(Default)]
    struct RecordingLauncher {
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl AppLauncher for RecordingLauncher {
        fn launch(&self, argv: &[String]) -> std::io::Result<()> {
            self.calls.lock().unwrap().push(argv.to_vec());
            Ok(())
        }
    }

    struct FailingLauncher;

    impl AppLauncher for FailingLauncher {
        fn launch(&self, _argv: &[String]) -> std::io::Result<()> {
            Err(io::Error::other("ambiguous spawn failure"))
        }
    }

    /// Write a minimal `.desktop` file into an XDG `applications` dir under `home`
    /// so `scan_local_apps` discovers it (the id is the file stem).
    fn seed_desktop_app(home: &Path, id: &str, exec: &str) {
        let dir = home.join(".local").join("share").join("applications");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{id}.desktop")),
            format!("[Desktop Entry]\nType=Application\nName={id}\nExec={exec}\n"),
        )
        .unwrap();
    }

    fn seed_flatpak_app(home: &Path, id: &str, exec: &str) {
        let dir = home
            .join(".local")
            .join("share/flatpak/exports/share/applications");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{id}.desktop")),
            format!("[Desktop Entry]\nType=Application\nName={id}\nExec={exec}\n"),
        )
        .unwrap();
    }

    fn worker_with(home: PathBuf, launcher: Arc<RecordingLauncher>) -> PeerAppLaunchWorker {
        PeerAppLaunchWorker::new("node-a".to_string())
            .with_home(home)
            .with_launcher(launcher)
    }

    fn launch_body(app_id: &str, nonce: &str) -> (String, tempfile::TempDir) {
        let auth_root = tempfile::tempdir().unwrap();
        (armed_launch_body(app_id, nonce), auth_root)
    }

    fn armed_launch_body(app_id: &str, nonce: &str) -> String {
        let unsigned = format!(
            r#"{{"node":"node-a","app_id":"{app_id}","name":"Test","source":"xdg","mode":"legacy-host","schema_version":1}}"#
        );
        authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: PEER_APP_LAUNCH_AUTH_VERB,
                node: "node-a",
                target: app_id,
            },
            nonce,
            AUTH_NOW + 30_000,
        )
    }

    fn worker_with_auth(
        home: PathBuf,
        launcher: Arc<RecordingLauncher>,
        auth_root: &Path,
    ) -> PeerAppLaunchWorker {
        worker_with(home, launcher).with_authorizer(Arc::new(ActionAuthorizer::for_test(
            AUTH_KEY,
            auth_root.to_path_buf(),
            AUTH_NOW,
        )))
    }

    fn replace_index(root: &Path, replacement: &Path) {
        for sidecar in ["index.sqlite-wal", "index.sqlite-shm"] {
            match fs::remove_file(root.join(sidecar)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => panic!("remove retired sidecar: {error}"),
            }
        }
        fs::rename(replacement, root.join("index.sqlite")).expect("install replacement index");
    }

    fn read_result(root: &Path, ulid: &str) -> LaunchResult {
        let body = Persist::open(root.to_path_buf())
            .unwrap()
            .read_latest(&launch_result_topic(ulid))
            .unwrap()
            .and_then(|message| message.body)
            .expect("typed launch result");
        serde_json::from_str(&body).expect("launch result schema")
    }

    #[test]
    fn parse_rejects_malformed_and_incomplete_requests() {
        assert!(parse_launch_request("not json").is_none());
        assert!(parse_launch_request(r#"{"node":"node-a"}"#).is_none());
        assert!(parse_launch_request(r#"{"app_id":"firefox"}"#).is_none());
        assert!(parse_launch_request(r#"{"node":"","app_id":"firefox"}"#).is_none());
        let ok = parse_launch_request(
            r#"{"node":"node-a","app_id":"firefox","name":"Firefox","source":"xdg","mode":"legacy-host"}"#,
        )
            .expect("valid request parses");
        assert_eq!(ok.node, "node-a");
        assert_eq!(ok.app_id, "firefox");
        assert_eq!(ok.name, "Firefox");
        assert_eq!(ok.source, LaunchSource::Xdg);
        assert_eq!(ok.mode, LaunchMode::LegacyHost);
        assert!(ok.session_id.is_none());
    }

    #[test]
    fn launch_argv_strips_field_codes_and_env_prefix() {
        assert_eq!(launch_argv("firefox %U"), Some(vec!["firefox".to_string()]));
        assert_eq!(
            launch_argv("env GDK_BACKEND=x11 /usr/bin/app --flag %F"),
            Some(vec!["/usr/bin/app".to_string(), "--flag".to_string()])
        );
        assert_eq!(launch_argv("%U"), None);
        assert_eq!(launch_argv("   "), None);
    }

    #[test]
    fn resolves_and_launches_a_known_advertised_app() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        seed_desktop_app(&home, "firefox", "firefox %U");
        let launcher = Arc::new(RecordingLauncher::default());
        let worker = worker_with(home, Arc::clone(&launcher));

        let req = LaunchRequest {
            node: "node-a".to_string(),
            app_id: "firefox".to_string(),
            name: "Firefox".to_string(),
            source: LaunchSource::Xdg,
            mode: LaunchMode::LegacyHost,
            session_id: None,
            vm_id: None,
            catalog_revision: None,
            guest_profile: None,
            requested_capabilities: Vec::new(),
            resume: false,
            client_peer: None,
        };
        assert!(
            worker.handle_request(&req),
            "a known advertised app launches"
        );
        let calls = launcher.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], vec!["firefox".to_string()]);
    }

    #[test]
    fn legacy_host_launcher_rejects_guest_owned_flatpak() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        seed_flatpak_app(&home, "org.example.Guest", "flatpak run org.example.Guest");
        let launcher = Arc::new(RecordingLauncher::default());
        let worker = worker_with(home, Arc::clone(&launcher));

        let req = LaunchRequest {
            node: "node-a".to_string(),
            app_id: "org.example.Guest".to_string(),
            name: "Guest app".to_string(),
            source: LaunchSource::Flatpak,
            mode: LaunchMode::GuestAppVm,
            session_id: None,
            vm_id: None,
            catalog_revision: None,
            guest_profile: None,
            requested_capabilities: Vec::new(),
            resume: false,
            client_peer: None,
        };
        assert!(
            !worker.handle_request(&req),
            "guest-owned Flatpak must not fall through to the host launcher"
        );
        assert!(
            launcher.calls.lock().unwrap().is_empty(),
            "Flatpak policy refusal must happen before process spawn"
        );
    }

    #[test]
    fn guest_launch_maps_to_typed_app_vm_session_without_host_fallback() {
        let req = parse_launch_request(
            r#"{"node":"node-a","app_id":"org.example.Guest","name":"Guest","source":"flatpak","mode":"guest-app-vm","session_id":"sess-1","vm_id":"vm-1","catalog_revision":"catalog-7","guest_profile":"wayland-standard","requested_capabilities":["audio"],"client_peer":"peer:seat","resume":true}"#,
        )
        .expect("guest launch parses");
        assert_eq!(
            app_vm_session_request(&req),
            Some(SessionRequest::OpenApp {
                id: "sess-1".into(),
                serving_peer: "node-a".into(),
                vm_id: "vm-1".into(),
                client_peer: "peer:seat".into(),
                app_id: "org.example.Guest".into(),
                catalog_revision: "catalog-7".into(),
                guest_profile: "wayland-standard".into(),
                requested_capabilities: vec!["audio".into()],
                resume: true,
            })
        );
    }

    #[test]
    fn guest_launch_requires_explicit_lifecycle_identity() {
        let req = parse_launch_request(
            r#"{"node":"node-a","app_id":"org.example.Guest","source":"flatpak","mode":"guest-app-vm"}"#,
        )
        .expect("source and mode still parse");
        assert!(app_vm_session_request(&req).is_none());
    }

    #[test]
    fn guest_launch_rejects_capabilities_outside_admitted_policy() {
        let req = parse_launch_request(
            r#"{"node":"node-a","app_id":"org.example.Guest","source":"flatpak","mode":"guest-app-vm","session_id":"sess-1","vm_id":"vm-1","catalog_revision":"catalog-7","guest_profile":"wayland-standard","requested_capabilities":["host-files"],"client_peer":"peer:seat"}"#,
        )
        .expect("guest launch parses before policy admission");
        assert!(
            app_vm_session_request(&req).is_none(),
            "unsupported capabilities must not become an OpenApp lifecycle mutation"
        );
    }

    #[test]
    fn guest_launch_rejects_unadmitted_route_provenance_before_session_projection() {
        let admitted = parse_launch_request(
            r#"{"node":"node-a","app_id":"org.example.Guest","source":"flatpak","mode":"guest-app-vm","session_id":"sess-1","vm_id":"vm-1","catalog_revision":"catalog-7","guest_profile":"wayland-standard","requested_capabilities":["audio"],"client_peer":"peer:seat"}"#,
        )
        .expect("admitted route parses");
        for (field, value) in [
            ("node", "peer/serving"),
            ("vm_id", "vm\\substituted"),
            ("client_peer", "peer:\nseat"),
        ] {
            let mut request = admitted.clone();
            match field {
                "node" => request.node = value.into(),
                "vm_id" => request.vm_id = Some(value.into()),
                "client_peer" => request.client_peer = Some(value.into()),
                _ => unreachable!(),
            }
            assert!(
                app_vm_session_request(&request).is_none(),
                "unsafe {field} must not become an OpenApp lifecycle mutation"
            );
        }
    }

    #[test]
    fn legacy_host_launcher_preserves_xdg_catalog_allowlist() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        seed_desktop_app(&home, "firefox", "firefox %U");
        let launcher = Arc::new(RecordingLauncher::default());
        let worker = worker_with(home, Arc::clone(&launcher));

        let req = LaunchRequest {
            node: "node-a".to_string(),
            app_id: "firefox".to_string(),
            name: "Firefox".to_string(),
            source: LaunchSource::Xdg,
            mode: LaunchMode::LegacyHost,
            session_id: None,
            vm_id: None,
            catalog_revision: None,
            guest_profile: None,
            requested_capabilities: Vec::new(),
            resume: false,
            client_peer: None,
        };
        assert!(worker.handle_request(&req));
        let calls = launcher.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], vec!["firefox".to_string()]);
    }

    #[test]
    fn refuses_an_app_not_advertised_by_this_node() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        // Only 'firefox' is advertised; the request asks for something else.
        seed_desktop_app(&home, "firefox", "firefox %U");
        let launcher = Arc::new(RecordingLauncher::default());
        let worker = worker_with(home, Arc::clone(&launcher));

        let req = LaunchRequest {
            node: "node-a".to_string(),
            app_id: "rm-rf-everything".to_string(),
            name: "totally legit".to_string(),
            source: LaunchSource::Xdg,
            mode: LaunchMode::LegacyHost,
            session_id: None,
            vm_id: None,
            catalog_revision: None,
            guest_profile: None,
            requested_capabilities: Vec::new(),
            resume: false,
            client_peer: None,
        };
        assert!(
            !worker.handle_request(&req),
            "an id not in this node's catalog must be refused"
        );
        assert!(
            launcher.calls.lock().unwrap().is_empty(),
            "a refused request must NEVER exec anything"
        );
    }

    #[test]
    fn refuses_a_request_addressed_to_another_node() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        seed_desktop_app(&home, "firefox", "firefox %U");
        let launcher = Arc::new(RecordingLauncher::default());
        let worker = worker_with(home, Arc::clone(&launcher));

        let req = LaunchRequest {
            node: "some-other-node".to_string(),
            app_id: "firefox".to_string(),
            name: "Firefox".to_string(),
            source: LaunchSource::Xdg,
            mode: LaunchMode::LegacyHost,
            session_id: None,
            vm_id: None,
            catalog_revision: None,
            guest_profile: None,
            requested_capabilities: Vec::new(),
            resume: false,
            client_peer: None,
        };
        assert!(
            !worker.handle_request(&req),
            "a request for another node must not act here"
        );
        assert!(launcher.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn the_wire_never_supplies_the_command() {
        // Even if a request's fields try to smuggle a command, only the catalog's
        // own Exec line is ever run. Here the advertised app's real Exec is a fixed
        // binary; the request's app_id merely selects it, and no request field can
        // change the argv.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        seed_desktop_app(&home, "safe-app", "/usr/bin/safe-app --managed");
        let launcher = Arc::new(RecordingLauncher::default());
        let worker = worker_with(home, Arc::clone(&launcher));

        let req = LaunchRequest {
            node: "node-a".to_string(),
            app_id: "safe-app".to_string(),
            name: "rm -rf / ; evil".to_string(),
            source: LaunchSource::Xdg,
            mode: LaunchMode::LegacyHost,
            session_id: None,
            vm_id: None,
            catalog_revision: None,
            guest_profile: None,
            requested_capabilities: Vec::new(),
            resume: false,
            client_peer: None,
        };
        assert!(worker.handle_request(&req));
        let calls = launcher.calls.lock().unwrap();
        assert_eq!(
            calls[0],
            vec!["/usr/bin/safe-app".to_string(), "--managed".to_string()],
            "the argv comes ONLY from the resolved catalog entry, never the request",
        );
    }

    #[test]
    fn unsigned_launch_is_refused_before_spawn() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        seed_desktop_app(&home, "firefox", "firefox %U");
        let launcher = Arc::new(RecordingLauncher::default());
        let auth_root = tempfile::tempdir().unwrap();
        let worker = worker_with_auth(home, Arc::clone(&launcher), auth_root.path());

        assert!(!worker.handle_body(
            r#"{"node":"node-a","app_id":"firefox","name":"Firefox","schema_version":1}"#
        ));
        assert!(launcher.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn exact_body_launch_is_single_use_and_tamper_does_not_consume_nonce() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        seed_desktop_app(&home, "firefox", "firefox %U");
        let launcher = Arc::new(RecordingLauncher::default());
        let (armed, auth_root) = launch_body("firefox", "launch-once");
        let worker = worker_with_auth(home, Arc::clone(&launcher), auth_root.path());

        let tampered = armed.replace("Test", "Tampered");
        assert!(
            !worker.handle_body(&tampered),
            "body tampering must be refused"
        );
        assert!(
            worker.handle_body(&armed),
            "the untouched capability remains valid"
        );
        assert!(
            !worker.handle_body(&armed),
            "the capability nonce is single-use"
        );
        assert_eq!(launcher.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn bus_r87_retained_action_is_skipped_and_first_forward_action_launches_once() {
        let temp = tempfile::tempdir().unwrap();
        let bus_root = temp.path().join("bus");
        let state_root = temp.path().join("state");
        let home = temp.path().join("home");
        let auth_root = temp.path().join("auth");
        seed_desktop_app(&home, "firefox", "firefox %U");
        let persist = Persist::open(bus_root.clone()).unwrap();
        let retained = persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&armed_launch_body("firefox", "retained-action")),
            )
            .unwrap();
        let launcher = Arc::new(RecordingLauncher::default());
        let worker = worker_with_auth(home, Arc::clone(&launcher), &auth_root)
            .with_state_root(state_root.clone());
        let mut journal = LaunchJournal::open(&state_root).unwrap();
        let transaction = BusTransaction::open(&bus_root).unwrap();
        let mut cursor = worker
            .activate_bus(&transaction, &mut journal)
            .unwrap()
            .expect("activation complete");
        assert_eq!(cursor.as_deref(), Some(retained.ulid.as_str()));

        let forward = persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&armed_launch_body("firefox", "first-forward")),
            )
            .unwrap();
        worker
            .process_page(&transaction, &mut journal, &mut cursor)
            .unwrap();
        worker
            .process_page(&transaction, &mut journal, &mut cursor)
            .unwrap();

        assert_eq!(launcher.calls.lock().unwrap().len(), 1);
        assert_eq!(
            read_result(&bus_root, &forward.ulid).status,
            LaunchResultStatus::Succeeded
        );
        assert!(Persist::open(bus_root)
            .unwrap()
            .read_latest(&launch_result_topic(&retained.ulid))
            .unwrap()
            .is_none());
    }

    #[test]
    fn bus_r87_same_path_replacement_after_read_retires_page_and_preserves_first_forward() {
        let temp = tempfile::tempdir().unwrap();
        let bus_root = temp.path().join("bus");
        let replacement_root = temp.path().join("replacement");
        let state_root = temp.path().join("state");
        let home = temp.path().join("home");
        let auth_root = temp.path().join("auth");
        seed_desktop_app(&home, "firefox", "firefox %U");
        let original = Persist::open(bus_root.clone()).unwrap();
        let replacement = Persist::open(replacement_root.clone()).unwrap();
        let retained_replacement = replacement
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&armed_launch_body("firefox", "replacement-retained")),
            )
            .unwrap();
        drop(replacement);
        let launcher = Arc::new(RecordingLauncher::default());
        let replaced = Arc::new(AtomicBool::new(false));
        let replace_once = Arc::clone(&replaced);
        let replacement_index = replacement_root.join("index.sqlite");
        let worker = worker_with_auth(home, Arc::clone(&launcher), &auth_root)
            .with_state_root(state_root.clone())
            .with_after_action_read(move |root| {
                if !replace_once.swap(true, Ordering::SeqCst) {
                    replace_index(root, &replacement_index);
                }
            });
        let mut journal = LaunchJournal::open(&state_root).unwrap();
        let retired = BusTransaction::open(&bus_root).unwrap();
        let mut cursor = worker
            .activate_bus(&retired, &mut journal)
            .unwrap()
            .expect("initial activation");
        original
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&armed_launch_body("firefox", "retired-forward")),
            )
            .unwrap();
        assert!(worker
            .process_page(&retired, &mut journal, &mut cursor)
            .is_err());
        assert!(launcher.calls.lock().unwrap().is_empty());

        let current = BusTransaction::open(&bus_root).unwrap();
        cursor = worker
            .activate_bus(&current, &mut journal)
            .unwrap()
            .expect("replacement activation");
        assert_eq!(cursor.as_deref(), Some(retained_replacement.ulid.as_str()));
        let forward = Persist::open(bus_root.clone())
            .unwrap()
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&armed_launch_body("firefox", "replacement-forward")),
            )
            .unwrap();
        worker
            .process_page(&current, &mut journal, &mut cursor)
            .unwrap();
        assert_eq!(launcher.calls.lock().unwrap().len(), 1);
        assert_eq!(
            read_result(&bus_root, &forward.ulid).status,
            LaunchResultStatus::Succeeded
        );
    }

    #[test]
    fn bus_r87_replacement_after_result_write_corrects_forward_without_repeating_launch() {
        let temp = tempfile::tempdir().unwrap();
        let bus_root = temp.path().join("bus");
        let replacement_root = temp.path().join("replacement");
        let state_root = temp.path().join("state");
        let home = temp.path().join("home");
        let auth_root = temp.path().join("auth");
        seed_desktop_app(&home, "firefox", "firefox %U");
        let original = Persist::open(bus_root.clone()).unwrap();
        drop(Persist::open(replacement_root.clone()).unwrap());
        let launcher = Arc::new(RecordingLauncher::default());
        let replaced = Arc::new(AtomicBool::new(false));
        let replace_once = Arc::clone(&replaced);
        let replacement_index = replacement_root.join("index.sqlite");
        let worker = worker_with_auth(home, Arc::clone(&launcher), &auth_root)
            .with_state_root(state_root.clone())
            .with_after_result_write(move |root| {
                if !replace_once.swap(true, Ordering::SeqCst) {
                    replace_index(root, &replacement_index);
                }
            });
        let mut journal = LaunchJournal::open(&state_root).unwrap();
        let retired = BusTransaction::open(&bus_root).unwrap();
        let mut cursor = worker
            .activate_bus(&retired, &mut journal)
            .unwrap()
            .expect("activation");
        let action = original
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&armed_launch_body("firefox", "result-race")),
            )
            .unwrap();
        assert!(worker
            .process_page(&retired, &mut journal, &mut cursor)
            .is_err());
        assert_eq!(launcher.calls.lock().unwrap().len(), 1);

        let current = BusTransaction::open(&bus_root).unwrap();
        assert!(worker
            .activate_bus(&current, &mut journal)
            .unwrap()
            .is_some());
        assert_eq!(launcher.calls.lock().unwrap().len(), 1);
        assert_eq!(
            read_result(&bus_root, &action.ulid).status,
            LaunchResultStatus::Succeeded
        );
    }

    #[test]
    fn bus_r87_recovered_effect_claim_and_spawn_error_are_indeterminate_never_success() {
        let temp = tempfile::tempdir().unwrap();
        let bus_root = temp.path().join("bus");
        let state_root = temp.path().join("state");
        let home = temp.path().join("home");
        let auth_root = temp.path().join("auth");
        seed_desktop_app(&home, "firefox", "firefox %U");
        drop(Persist::open(bus_root.clone()).unwrap());
        let worker = worker_with_auth(home, Arc::new(RecordingLauncher::default()), &auth_root)
            .with_state_root(state_root.clone());
        let mut journal = LaunchJournal::open(&state_root).unwrap();
        journal
            .insert_prepared(
                "claimed-before-crash",
                armed_launch_body("firefox", "claimed-before-crash"),
                "node-a".into(),
                "firefox".into(),
                vec!["firefox".into()],
            )
            .unwrap();
        journal
            .set_phase("claimed-before-crash", LaunchJournalPhase::EffectClaimed)
            .unwrap();
        drop(journal);
        let mut recovered = LaunchJournal::open(&state_root).unwrap();
        let transaction = BusTransaction::open(&bus_root).unwrap();
        worker
            .activate_bus(&transaction, &mut recovered)
            .unwrap()
            .expect("recovery activation");
        assert_eq!(
            read_result(&bus_root, "claimed-before-crash").status,
            LaunchResultStatus::Indeterminate
        );

        let failure_home = temp.path().join("failure-home");
        seed_desktop_app(&failure_home, "firefox", "firefox %U");
        let failing = PeerAppLaunchWorker::new("node-a".into())
            .with_home(failure_home)
            .with_launcher(Arc::new(FailingLauncher))
            .with_authorizer(Arc::new(ActionAuthorizer::for_test(
                AUTH_KEY,
                auth_root.clone(),
                AUTH_NOW,
            )))
            .with_state_root(temp.path().join("failure-state"));
        let mut failure_journal = LaunchJournal::open(&failing.state_root).unwrap();
        let mut cursor = failing
            .activate_bus(&transaction, &mut failure_journal)
            .unwrap()
            .expect("failure activation");
        let action = Persist::open(bus_root.clone())
            .unwrap()
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&armed_launch_body("firefox", "spawn-error")),
            )
            .unwrap();
        failing
            .process_page(&transaction, &mut failure_journal, &mut cursor)
            .unwrap();
        assert_eq!(
            read_result(&bus_root, &action.ulid).status,
            LaunchResultStatus::Indeterminate
        );
    }
}
