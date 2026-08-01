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

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use mde_bus::persist::Persist;

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};
use crate::ipc::apps::{default_app_dirs, scan_local_apps, AppEntry};
use mackes_mesh_types::vdi_session::{AppVmLaunchRequest, SessionRequest};

use super::{ShutdownToken, Worker};

/// The flat Bus topic this worker drains. Per-node targeting is via the request's
/// `node` field, not the topic (the same shape [`crate::workers::container`] uses).
pub const ACTION_TOPIC: &str = "action/apps/launch";

/// Action-drain cadence. The bus read is a cheap local log scan; a launch is a
/// rare, operator-initiated event, so a 1 s poll is responsive without spinning.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Shared-Bus capability context for the remote app-launch mutation. The
/// worker binds the capability to this node (the only node whose catalog it
/// may resolve) and the requested catalog id.
const PEER_APP_LAUNCH_AUTH_VERB: &str = "peer-app-launch";

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
    AppVmLaunchRequest::new(
        req.app_id.clone(),
        catalog_revision.clone(),
        guest_profile.clone(),
        req.requested_capabilities.clone(),
        session_id.clone(),
        req.resume,
    )
    .ok()?;
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
        let (bin, args) = argv
            .split_first()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty argv"))?;
        Command::new(bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_child| ())
    }
}

// ───────────────────────────── bus plumbing ─────────────────────────────

/// Read new [`ACTION_TOPIC`] messages since `cursor`, advancing it. A short sync
/// open-read-drop (never crosses an `.await`), mirroring [`crate::workers::container`].
fn read_new_requests(bus_root: &Path, cursor: &mut Option<String>) -> Vec<String> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(msgs) = persist.list_since(ACTION_TOPIC, cursor.as_deref()) else {
        return vec![];
    };
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        out.push(msg.body.unwrap_or_default());
    }
    out
}

/// Seed the cursor to the newest existing message so a (re)start never re-launches
/// the backlog — a queued launch must not re-fire on the next daemon restart.
fn prime_cursor(bus_root: &Path) -> Option<String> {
    let persist = Persist::open(bus_root.to_path_buf()).ok()?;
    let msgs = persist.list_since(ACTION_TOPIC, None).ok()?;
    msgs.last().map(|m| m.ulid.clone())
}

fn default_bus_root() -> Option<PathBuf> {
    mde_bus::default_data_dir()
}

fn default_home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from)
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
    /// Shared, fail-closed authorization gate for the remote launch mutation.
    authorizer: Arc<ActionAuthorizer>,
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
            authorizer: Arc::new(ActionAuthorizer::production()),
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

    /// Drain + handle new requests addressed to this node. Returns whether any app
    /// launched (for the caller's own bookkeeping / tests).
    fn drain_and_launch(&self, bus_root: &Path, cursor: &mut Option<String>) -> bool {
        let mut launched = false;
        for body in read_new_requests(bus_root, cursor) {
            launched |= self.handle_body(&body);
        }
        launched
    }
}

#[async_trait::async_trait]
impl Worker for PeerAppLaunchWorker {
    fn name(&self) -> &'static str {
        "peer_app_launch"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = self.bus_root();
        // Skip any backlog so a restart doesn't re-launch stale requests.
        let mut cursor = bus_root.as_deref().and_then(prime_cursor);
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if let Some(root) = &bus_root {
                        let _ = self.drain_and_launch(root, &mut cursor);
                    }
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer};
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
        let unsigned = format!(
            r#"{{"node":"node-a","app_id":"{app_id}","name":"Test","source":"xdg","mode":"legacy-host","schema_version":1}}"#
        );
        let armed = authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: PEER_APP_LAUNCH_AUTH_VERB,
                node: "node-a",
                target: app_id,
            },
            nonce,
            AUTH_NOW + 30_000,
        );
        (armed, auth_root)
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
}
