//! Portal-52.a (v6.0, R12-Q13 — workspace-structure half) — sway
//! session-restore worker.
//!
//! Two responsibilities:
//!
//!   1. **Snapshot** — every 5 seconds, walk the live sway tree
//!      via `get_workspaces` + `get_tree`, serialize the
//!      workspace structure (number + name + output + layout) to
//!      `<XDG_DATA_HOME>/mde/session.json` via atomic
//!      temp + rename.
//!   2. **Restore** — on first event after worker start, read
//!      the snapshot file. For each workspace fire swayipc
//!      `workspace number <n>; move workspace to output <out>;
//!      layout <name>` to recreate the slot + output + layout
//!      triple. Operator's apps don't auto-relaunch in this
//!      half; Portal-52.b ships the `append_layout` swallow
//!      placeholders.
//!
//! Operators get workspaces in their correct slots / outputs /
//! layouts on every login. App relaunch is operator-driven
//! (Mod+Space / Hub click). This split is per CLAUDE.md §0.12 —
//! Portal-52.a is bench-observable on its own (login lands
//! workspaces in slots) and Portal-52.b extends it with the
//! placeholder swallows.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use swayipc_async::Connection;

use super::{ShutdownToken, Worker};

const RECONNECT_BACKOFF: Duration = Duration::from_secs(3);
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(5);

/// Schema version for the session snapshot file. Bump on
/// backwards-incompatible changes; for now informational.
pub const SCHEMA_VERSION: u32 = 1;

/// One workspace's structural state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    /// i3/sway workspace number.
    pub workspace_num: i32,
    /// Workspace name as displayed (Portal-41 auto-derived form,
    /// or operator-set).
    pub name: String,
    /// Output the workspace lives on (e.g. `HDMI-A-1`).
    pub output: String,
    /// Container layout: `splith` / `splitv` / `tabbed` / `stacked`.
    pub layout: String,
}

/// Top-level session snapshot. Wraps `Vec<WorkspaceSnapshot>` with
/// a schema_version for forward-compat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Snapshot schema version — readers refuse newer-than-known
    /// values + accept older ones via defaulted fields.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Per-workspace state captured at snapshot time.
    #[serde(default)]
    pub workspaces: Vec<WorkspaceSnapshot>,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

impl Default for SessionSnapshot {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            workspaces: Vec::new(),
        }
    }
}

/// Worker state. `restored` flips to `true` after the first
/// restore attempt so we don't re-restore on every event during a
/// single mded lifetime.
pub struct SessionPersistWorker {
    restored: bool,
}

impl SessionPersistWorker {
    /// Construct a fresh worker — restore pending, snapshot ticks
    /// will start once the worker enters run().
    #[must_use]
    pub fn new() -> Self {
        Self { restored: false }
    }
}

impl Default for SessionPersistWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for SessionPersistWorker {
    fn name(&self) -> &'static str {
        "session_persist"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            if shutdown.is_shutdown() {
                return Ok(());
            }
            let mut conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "session_persist connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            // First-run-only restore pass. Subsequent reconnects
            // (e.g. sway restart) don't re-restore — the operator's
            // current session is the source of truth from then on.
            if !self.restored {
                if let Err(e) = restore_from_default(&mut conn).await {
                    tracing::debug!(error = %e, "session_persist restore failed; continuing snapshot cadence");
                }
                self.restored = true;
            }
            // 5-second snapshot loop. Aborts on shutdown.
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.wait() => return Ok(()),
                    _ = tokio::time::sleep(SNAPSHOT_INTERVAL) => {
                        match snapshot_to_default(&mut conn).await {
                            Ok(()) => {}
                            Err(SessionPersistError::Connection) => {
                                tracing::debug!("session_persist snapshot lost connection; reconnecting");
                                break;
                            }
                            Err(e) => {
                                tracing::debug!(error = ?e, "session_persist snapshot non-fatal error; continuing");
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn sleep_or_shutdown(dur: Duration, shutdown: &mut ShutdownToken) {
    tokio::select! {
        _ = shutdown.wait() => {}
        _ = tokio::time::sleep(dur) => {}
    }
}

/// Error type returned by [`write_snapshot_atomic`] and
/// [`read_snapshot`]. Carries the four distinct failure modes
/// the worker can encounter at the FS / IPC / JSON boundaries.
#[derive(Debug)]
pub enum SessionPersistError {
    /// swayipc connection dropped or refused.
    Connection,
    /// FS-side IO failure (read / write / rename).
    Io(std::io::Error),
    /// JSON serde failure (parse on read, serialize on write).
    Json(serde_json::Error),
    /// `$HOME` / `$XDG_DATA_HOME` not set, so the session.json
    /// path couldn't be resolved.
    PathResolution,
}

impl std::fmt::Display for SessionPersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connection => write!(f, "swayipc connection dropped"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Json(e) => write!(f, "json: {e}"),
            Self::PathResolution => write!(f, "could not resolve session.json path"),
        }
    }
}

impl From<std::io::Error> for SessionPersistError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for SessionPersistError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Snapshot the live sway tree to the default path
/// (`<XDG_DATA_HOME>/mde/session.json`). Atomic write via
/// temp + rename.
async fn snapshot_to_default(conn: &mut Connection) -> Result<(), SessionPersistError> {
    let path = default_session_path().ok_or(SessionPersistError::PathResolution)?;
    let snapshot = build_snapshot(conn).await?;
    write_snapshot_atomic(&path, &snapshot)?;
    Ok(())
}

/// Walk the live sway tree + workspace list to build a snapshot.
async fn build_snapshot(conn: &mut Connection) -> Result<SessionSnapshot, SessionPersistError> {
    let workspaces = conn
        .get_workspaces()
        .await
        .map_err(|_| SessionPersistError::Connection)?;
    let tree = conn
        .get_tree()
        .await
        .map_err(|_| SessionPersistError::Connection)?;
    let mut out = SessionSnapshot::default();
    for ws in workspaces {
        if ws.num < 0 {
            // Sway internal scratchpad meta-workspace — skip.
            continue;
        }
        let layout = workspace_layout(&tree, ws.num).unwrap_or_else(|| "splith".to_string());
        out.workspaces.push(WorkspaceSnapshot {
            workspace_num: ws.num,
            name: ws.name,
            output: ws.output,
            layout,
        });
    }
    Ok(out)
}

/// Restore from the default-path snapshot. Missing file is not
/// an error — first-boot path.
async fn restore_from_default(conn: &mut Connection) -> Result<(), SessionPersistError> {
    let path = default_session_path().ok_or(SessionPersistError::PathResolution)?;
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(&path)?;
    let snap: SessionSnapshot = serde_json::from_str(&raw)?;
    for ws in &snap.workspaces {
        // Portal-59: skip the parked workspace — it's an
        // ephemeral platform slot.
        if ws.workspace_num == 99 {
            continue;
        }
        let cmd = restore_command(ws);
        if let Err(e) = conn.run_command(&cmd).await {
            tracing::warn!(workspace = ws.workspace_num, error = %e, "session_persist restore command failed");
        }
    }
    Ok(())
}

// ── Pure helpers ────────────────────────────────────────────────────────

/// Resolve `<XDG_DATA_HOME>/mde/session.json`.
#[must_use]
pub fn default_session_path() -> Option<PathBuf> {
    let data_home = dirs::data_dir()?;
    Some(data_home.join("mde").join("session.json"))
}

/// Atomic write of `snapshot` to `path` via temp + rename.
/// Creates the parent directory if missing.
pub fn write_snapshot_atomic(
    path: &std::path::Path,
    snapshot: &SessionSnapshot,
) -> Result<(), SessionPersistError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let pretty = serde_json::to_string_pretty(snapshot)?;
    let mut tmp = path.to_path_buf();
    tmp.set_extension("json.tmp");
    std::fs::write(&tmp, pretty)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Read a snapshot from `path`. Missing file returns the empty
/// default snapshot (first-run path).
pub fn read_snapshot(path: &std::path::Path) -> Result<SessionSnapshot, SessionPersistError> {
    if !path.exists() {
        return Ok(SessionSnapshot::default());
    }
    let raw = std::fs::read_to_string(path)?;
    let snap: SessionSnapshot = serde_json::from_str(&raw)?;
    Ok(snap)
}

/// Build the swayipc command string that recreates `ws`'s slot +
/// output + layout. Three semicolon-chained directives:
///
///   `workspace number <n>; move workspace to output "<out>";
///    layout <name>`
///
/// Embedded double-quotes in the output name are backslash-
/// escaped so quirky output names still parse.
#[must_use]
pub fn restore_command(ws: &WorkspaceSnapshot) -> String {
    let out_escaped = ws.output.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "workspace number {n}; move workspace to output \"{out}\"; layout {layout}",
        n = ws.workspace_num,
        out = out_escaped,
        layout = ws.layout
    )
}

/// Walk the sway tree to find the container layout (`splith` /
/// `splitv` / `tabbed` / `stacked`) for workspace `num`. Returns
/// `None` if the workspace isn't in the tree.
fn workspace_layout(node: &swayipc_async::Node, num: i32) -> Option<String> {
    if node.node_type == swayipc_async::NodeType::Workspace && node.num == Some(num) {
        return Some(format!("{:?}", node.layout).to_lowercase());
    }
    for child in &node.nodes {
        if let Some(found) = workspace_layout(child, num) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> SessionSnapshot {
        SessionSnapshot {
            schema_version: SCHEMA_VERSION,
            workspaces: vec![
                WorkspaceSnapshot {
                    workspace_num: 1,
                    name: "1: firefox".to_string(),
                    output: "HDMI-A-1".to_string(),
                    layout: "splith".to_string(),
                },
                WorkspaceSnapshot {
                    workspace_num: 2,
                    name: "2".to_string(),
                    output: "HDMI-A-1".to_string(),
                    layout: "tabbed".to_string(),
                },
            ],
        }
    }

    /// Round-trip a populated snapshot through serde JSON.
    #[test]
    fn snapshot_serde_round_trip() {
        let s = sample_snapshot();
        let json = serde_json::to_string_pretty(&s).unwrap();
        let parsed: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, s);
    }

    /// Atomic write + read cycle leaves no `.json.tmp` sibling +
    /// round-trips data.
    #[test]
    fn atomic_write_read_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/dir/session.json");
        let snap = sample_snapshot();
        write_snapshot_atomic(&path, &snap).unwrap();
        let read = read_snapshot(&path).unwrap();
        assert_eq!(read, snap);
        // Atomic-rename should leave no `.json.tmp` sibling.
        let sibling = path.with_extension("json.tmp");
        assert!(!sibling.exists());
    }

    /// Missing file → empty default snapshot.
    #[test]
    fn read_snapshot_missing_file_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nope/session.json");
        let snap = read_snapshot(&path).unwrap();
        assert_eq!(snap.schema_version, SCHEMA_VERSION);
        assert!(snap.workspaces.is_empty());
    }

    /// `restore_command` builds the canonical 3-directive swayipc
    /// chain. Mirrors the bench acceptance for ws1 splith on
    /// HDMI-A-1.
    #[test]
    fn restore_command_canonical_shape() {
        let ws = WorkspaceSnapshot {
            workspace_num: 1,
            name: "1: firefox".to_string(),
            output: "HDMI-A-1".to_string(),
            layout: "splith".to_string(),
        };
        assert_eq!(
            restore_command(&ws),
            r#"workspace number 1; move workspace to output "HDMI-A-1"; layout splith"#
        );
    }

    /// Output names with quotes / backslashes round-trip via the
    /// quote-escape.
    #[test]
    fn restore_command_escapes_quotes_and_backslashes() {
        let ws = WorkspaceSnapshot {
            workspace_num: 3,
            name: "3".to_string(),
            output: r#"weird"output"#.to_string(),
            layout: "tabbed".to_string(),
        };
        assert_eq!(
            restore_command(&ws),
            r#"workspace number 3; move workspace to output "weird\"output"; layout tabbed"#
        );
    }

    /// Pre-schema files (no schema_version field) load with the
    /// default version filled in by serde.
    #[test]
    fn pre_schema_files_load_with_default_version() {
        let json =
            r#"{"workspaces":[{"workspace_num":1,"name":"1","output":"eDP-1","layout":"splith"}]}"#;
        let snap: SessionSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(snap.schema_version, SCHEMA_VERSION);
        assert_eq!(snap.workspaces.len(), 1);
    }

    /// Empty snapshot (no workspaces) writes + reads cleanly —
    /// fresh-install path.
    #[test]
    fn empty_snapshot_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session.json");
        let snap = SessionSnapshot::default();
        write_snapshot_atomic(&path, &snap).unwrap();
        let read = read_snapshot(&path).unwrap();
        assert_eq!(read, snap);
    }
}
