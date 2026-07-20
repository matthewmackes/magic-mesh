//! WL-UX-005 — `peer_app_launch`: the peer-app remote-execution executor.
//!
//! The shell's unified Front Door lets an operator pick an app a *peer* node
//! advertises and launch it there. Front Door only PUBLISHES that intent — a
//! fire-and-forget `action/apps/launch` message carrying `{node, app_id, name}`
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

use crate::ipc::apps::{default_app_dirs, scan_local_apps, AppEntry};

use super::{ShutdownToken, Worker};

/// The flat Bus topic this worker drains. Per-node targeting is via the request's
/// `node` field, not the topic (the same shape [`crate::workers::container`] uses).
pub const ACTION_TOPIC: &str = "action/apps/launch";

/// Action-drain cadence. The bus read is a cheap local log scan; a launch is a
/// rare, operator-initiated event, so a 1 s poll is responsive without spinning.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

// ───────────────────────────── request model ─────────────────────────────

/// A parsed `action/apps/launch` request. Only the three wire fields the shell's
/// `front_door::peer_app_launch_wire` publishes; `app_id` is an opaque catalog id,
/// NEVER a command line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchRequest {
    /// The target node this launch is addressed to.
    pub node: String,
    /// The `.desktop` catalog id to resolve against the node's own app list.
    pub app_id: String,
    /// The display name (advisory — for logs only; never used to resolve).
    pub name: String,
}

impl LaunchRequest {
    /// Whether this request is addressed to `node_id`. An empty target never
    /// matches — the worker refuses to guess which node a launch is for.
    #[must_use]
    pub fn targets(&self, node_id: &str) -> bool {
        !self.node.is_empty() && self.node == node_id
    }
}

/// Parse one `action/apps/launch` body. `None` for non-JSON or a request missing a
/// `node` or `app_id` — a malformed request is refused, never guessed.
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
    Some(LaunchRequest {
        node: node.to_owned(),
        app_id: app_id.to_owned(),
        name,
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
fn read_new_requests(bus_root: &Path, cursor: &mut Option<String>) -> Vec<LaunchRequest> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(msgs) = persist.list_since(ACTION_TOPIC, cursor.as_deref()) else {
        return vec![];
    };
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_launch_request(body) {
            Some(req) => out.push(req),
            None => tracing::warn!(
                target: "mackesd::peer_app_launch",
                ulid = %msg.ulid,
                "peer_app_launch: malformed launch request refused (no node/app_id)",
            ),
        }
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
    Some(dirs::data_dir()?.join("mde").join("bus"))
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

    /// Drain + handle new requests addressed to this node. Returns whether any app
    /// launched (for the caller's own bookkeeping / tests).
    fn drain_and_launch(&self, bus_root: &Path, cursor: &mut Option<String>) -> bool {
        let mut launched = false;
        for req in read_new_requests(bus_root, cursor) {
            launched |= self.handle_request(&req);
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
    use std::sync::Mutex;

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

    fn worker_with(home: PathBuf, launcher: Arc<RecordingLauncher>) -> PeerAppLaunchWorker {
        PeerAppLaunchWorker::new("node-a".to_string())
            .with_home(home)
            .with_launcher(launcher)
    }

    #[test]
    fn parse_rejects_malformed_and_incomplete_requests() {
        assert!(parse_launch_request("not json").is_none());
        assert!(parse_launch_request(r#"{"node":"node-a"}"#).is_none());
        assert!(parse_launch_request(r#"{"app_id":"firefox"}"#).is_none());
        assert!(parse_launch_request(r#"{"node":"","app_id":"firefox"}"#).is_none());
        let ok = parse_launch_request(r#"{"node":"node-a","app_id":"firefox","name":"Firefox"}"#)
            .expect("valid request parses");
        assert_eq!(ok.node, "node-a");
        assert_eq!(ok.app_id, "firefox");
        assert_eq!(ok.name, "Firefox");
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
        };
        assert!(worker.handle_request(&req));
        let calls = launcher.calls.lock().unwrap();
        assert_eq!(
            calls[0],
            vec!["/usr/bin/safe-app".to_string(), "--managed".to_string()],
            "the argv comes ONLY from the resolved catalog entry, never the request",
        );
    }
}
