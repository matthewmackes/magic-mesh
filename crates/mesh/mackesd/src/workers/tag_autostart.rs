//! Portal-54 (v6.0, R12-Q16) — per-tag autostart on first
//! workspace::init.
//!
//! Subscribes to sway's `EventType::Workspace`. On every
//! `WorkspaceChange::Init`, the worker:
//!
//!   1. Looks up the owning tag for the new workspace (Portal-18.a
//!      schema).
//!   2. If the tag has a non-empty `autostart: Vec<String>`, AND
//!      the worker hasn't already fired autostart for that
//!      workspace number this lifetime, fires `swaymsg exec <cmd>`
//!      for each app_id in order.
//!
//! "Per mded-lifetime" means the worker tracks which workspaces
//! it has already autostarted in-process; restarting mded (or the
//! supervisor restarting the worker on failure) resets the set.
//! Sway-restart equivalents (operator typed `swaymsg reload`)
//! similarly emit fresh `workspace::init` events for surviving
//! workspaces — the worker re-fires autostart for those, matching
//! the design lock ("first init of a tag-owned workspace per
//! mded-lifetime").
//!
//! NOT XDG-autostart-compliant — tag-driven autostart is the only
//! mechanism per Portal-54's design body.

#![cfg(feature = "async-services")]

use std::collections::HashSet;
use std::time::Duration;

use futures_util::StreamExt as _;
use mackes_mesh_types::TagStore;
use swayipc_async::{Connection, EventType};

use super::workspace_router::find_owning_tag;
use super::{ShutdownToken, Worker};

const RECONNECT_BACKOFF: Duration = Duration::from_secs(3);

/// Worker that tracks which workspace numbers have already
/// received autostart commands.
pub struct TagAutostartWorker {
    seen: HashSet<i32>,
}

impl TagAutostartWorker {
    /// Construct a fresh worker with empty seen-set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            seen: HashSet::new(),
        }
    }
}

impl Default for TagAutostartWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for TagAutostartWorker {
    fn name(&self) -> &'static str {
        "tag_autostart"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            if shutdown.is_shutdown() {
                return Ok(());
            }
            let mut cmd_conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "tag_autostart cmd-conn connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            let event_conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "tag_autostart event-conn connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            let mut events = match event_conn.subscribe([EventType::Workspace]).await {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::debug!(error = %e, "tag_autostart subscribe failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.wait() => return Ok(()),
                    next = events.next() => {
                        match next {
                            Some(Ok(swayipc_async::Event::Workspace(ws_ev))) => {
                                if ws_ev.change == swayipc_async::WorkspaceChange::Init {
                                    if let Some(node) = ws_ev.current.as_ref() {
                                        if let Some(num) = node.num {
                                            self.handle_init(&mut cmd_conn, num).await;
                                        }
                                    }
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                tracing::debug!(error = %e, "tag_autostart event stream errored; reconnecting");
                                break;
                            }
                            None => {
                                tracing::debug!("tag_autostart event stream ended; reconnecting");
                                break;
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

impl TagAutostartWorker {
    /// Fire autostart for workspace `num` if its owning tag has a
    /// non-empty `autostart` list AND we haven't already fired
    /// for this workspace.
    async fn handle_init(&mut self, conn: &mut Connection, num: i32) {
        if !self.should_fire(num) {
            return;
        }
        let store = match TagStore::load_default() {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(error = %e, "tag_autostart tag-store load failed; skipping");
                return;
            }
        };
        let Some(owning) = find_owning_tag(&store, num) else {
            return;
        };
        // HYP-11.sway-bridge — tag-manifest `autostart` (bool) +
        // `apps` (Vec<String>) take precedence over TagStore's
        // legacy `autostart: Vec<String>`. When the manifest is
        // present + carries autostart=true, the effective list is
        // manifest.apps; autostart=false means "no autostart"
        // regardless of TagStore's setting (the manifest is the
        // source of truth per HYP-8.5).
        let manifest = crate::config::default_manifests_dir()
            .and_then(|d| crate::config::load_tag_manifests(&d).ok())
            .and_then(|ms| ms.into_iter().find(|m| m.name == owning.name));
        let effective_apps = effective_autostart_list(manifest.as_ref(), &owning.autostart);
        if effective_apps.is_empty() {
            return;
        }
        self.seen.insert(num);
        for app_id in &effective_apps {
            let cmd = exec_command(app_id);
            match conn.run_command(&cmd).await {
                Ok(_) => tracing::debug!(workspace = num, %app_id, "tag_autostart fired"),
                Err(e) => {
                    tracing::warn!(workspace = num, %app_id, error = %e, "tag_autostart exec failed")
                }
            }
        }
    }

    /// Return `true` if we haven't yet fired autostart for `num`.
    /// Pure-ish helper for tests — also flips the bit so calling
    /// twice in a row returns `(true, false)`.
    pub fn should_fire(&self, num: i32) -> bool {
        !self.seen.contains(&num)
    }

    /// Test-only seed of the seen set.
    #[cfg(test)]
    pub fn mark_seen(&mut self, num: i32) {
        self.seen.insert(num);
    }
}

/// Build the swayipc `exec` command string for `app_id`. Wraps
/// the command in shell-friendly double quotes with backslash
/// escapes so app_ids that happen to contain spaces or quotes
/// (rare but possible — e.g. `/usr/bin/foo bar`) still parse.
#[must_use]
pub fn exec_command(app_id: &str) -> String {
    let escaped = app_id.replace('\\', "\\\\").replace('"', "\\\"");
    format!("exec \"{escaped}\"")
}

/// HYP-11.sway-bridge — resolve the effective autostart list.
///
/// Precedence:
///
/// 1. **Manifest present + `autostart = true`** → return
///    `manifest.apps.clone()`. The manifest is the source of
///    truth per HYP-8.5; the `apps` Vec is the launch list.
/// 2. **Manifest present + `autostart = false`** → return empty
///    Vec. The operator explicitly opted out; do NOT fall through
///    to TagStore (that would be surprising behavior).
/// 3. **No manifest** → return `tagstore_autostart.to_vec()` as
///    the legacy Portal-18.a fallback.
///
/// Pure-fn — exposed as `pub` so the contract can be tested
/// without a sway connection.
#[must_use]
pub fn effective_autostart_list(
    manifest: Option<&crate::config::TagManifest>,
    tagstore_autostart: &[String],
) -> Vec<String> {
    match manifest {
        Some(m) => {
            if m.autostart {
                m.apps.clone()
            } else {
                Vec::new()
            }
        }
        None => tagstore_autostart.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_command_escapes_quotes_and_backslashes() {
        assert_eq!(exec_command("helix"), r#"exec "helix""#);
        assert_eq!(
            exec_command(r#"firefox "tab title""#),
            r#"exec "firefox \"tab title\"""#
        );
        assert_eq!(
            exec_command(r"path\with\slashes"),
            r#"exec "path\\with\\slashes""#
        );
    }

    #[test]
    fn should_fire_returns_true_for_unseen_workspace() {
        let w = TagAutostartWorker::new();
        assert!(w.should_fire(1));
        assert!(w.should_fire(2));
    }

    #[test]
    fn should_fire_returns_false_for_seen_workspace() {
        let mut w = TagAutostartWorker::new();
        w.mark_seen(1);
        assert!(!w.should_fire(1));
        // Other workspaces still fire-able.
        assert!(w.should_fire(2));
    }

    #[test]
    fn worker_starts_with_empty_seen_set() {
        let w = TagAutostartWorker::new();
        // No workspace number should be "seen" at construction.
        for n in -5..=10 {
            assert!(w.should_fire(n), "workspace {n} should be unseen");
        }
    }

    // ── HYP-11.sway-bridge — tag-manifest autostart precedence ──

    fn dev_manifest(autostart: bool, apps: &[&str]) -> crate::config::TagManifest {
        crate::config::TagManifest {
            name: "Dev".to_string(),
            autostart,
            apps: apps.iter().map(|s| s.to_string()).collect(),
            ..crate::config::TagManifest::default()
        }
    }

    /// Manifest present + autostart=true → returns manifest.apps.
    #[test]
    fn manifest_autostart_true_returns_manifest_apps() {
        let m = dev_manifest(true, &["foot", "code"]);
        let tagstore = vec!["legacy".to_string()];
        let r = effective_autostart_list(Some(&m), &tagstore);
        assert_eq!(r, vec!["foot".to_string(), "code".to_string()]);
    }

    /// Manifest present + autostart=false → empty (NOT TagStore).
    #[test]
    fn manifest_autostart_false_returns_empty_no_fallback() {
        let m = dev_manifest(false, &["foot", "code"]);
        let tagstore = vec!["legacy".to_string()];
        let r = effective_autostart_list(Some(&m), &tagstore);
        assert!(
            r.is_empty(),
            "false opt-out shouldn't fall through to TagStore"
        );
    }

    /// Manifest present + autostart=true + empty apps → empty.
    #[test]
    fn manifest_autostart_true_with_empty_apps_is_empty() {
        let m = dev_manifest(true, &[]);
        let tagstore = vec!["legacy".to_string()];
        let r = effective_autostart_list(Some(&m), &tagstore);
        assert!(r.is_empty());
    }

    /// No manifest → TagStore fallback.
    #[test]
    fn no_manifest_falls_through_to_tagstore() {
        let tagstore = vec!["foot".to_string(), "code".to_string()];
        let r = effective_autostart_list(None, &tagstore);
        assert_eq!(r, tagstore);
    }

    /// No manifest + empty TagStore → empty.
    #[test]
    fn no_manifest_with_empty_tagstore_is_empty() {
        let r = effective_autostart_list(None, &[]);
        assert!(r.is_empty());
    }
}
