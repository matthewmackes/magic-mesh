//! Portal-44 (v6.0, R12-Q4) — per-tag default_layout enforcement.
//!
//! Subscribes to sway's `EventType::Window`. On every
//! `WindowChange::New` event, the worker checks:
//!
//!   1. Is the new window's workspace owned by a tag?
//!   2. Does the owning tag have a `default_layout` set?
//!   3. Is this the only window currently on that workspace?
//!
//! If all three are true AND the workspace's current container
//! layout differs from the tag default, fires swayipc
//! `[con_id=<n>] focus; layout <name>` to flip the layout. The
//! second condition (single window) is the design lock —
//! flipping layout on every subsequent window would override
//! operator choice mid-session. Only the FIRST window in a tag-
//! owned workspace triggers the rule.
//!
//! Hub right-click 'Layout Chooser' UI (R3-Q62) is deferred to
//! Portal-18.b — operators set `default_layout` by hand-editing
//! `~/.local/share/mde/tags.json` until the modal lands.

#![cfg(feature = "async-services")]

use std::time::Duration;

use futures_util::StreamExt as _;
use mackes_mesh_types::{TagStore, WorkspaceOverridesFile};
use swayipc_async::{Connection, EventType};

use super::workspace_router::find_owning_tag;
use super::{ShutdownToken, Worker};

const RECONNECT_BACKOFF: Duration = Duration::from_secs(3);

/// Empty-state worker — tag store reloads per-event.
pub struct TagLayoutWorker;

impl TagLayoutWorker {
    /// Construct a fresh worker.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for TagLayoutWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for TagLayoutWorker {
    fn name(&self) -> &'static str {
        "tag_layout"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            if shutdown.is_shutdown() {
                return Ok(());
            }
            let mut cmd_conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "tag_layout cmd-conn connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            let event_conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "tag_layout event-conn connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            let mut events = match event_conn.subscribe([EventType::Window]).await {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::debug!(error = %e, "tag_layout subscribe failed; backing off");
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
                            Some(Ok(swayipc_async::Event::Window(win_ev))) => {
                                if win_ev.change == swayipc_async::WindowChange::New {
                                    handle_new_window(&mut cmd_conn, &win_ev.container).await;
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                tracing::debug!(error = %e, "tag_layout event stream errored; reconnecting");
                                break;
                            }
                            None => {
                                tracing::debug!("tag_layout event stream ended; reconnecting");
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

async fn handle_new_window(conn: &mut Connection, container: &swayipc_async::Node) {
    let con_id = container.id;
    // Re-fetch the tree to find the workspace this window landed
    // on + count siblings (the event's container alone doesn't
    // tell us workspace context).
    let tree = match conn.get_tree().await {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(error = %e, "tag_layout get_tree failed; skipping event");
            return;
        }
    };
    let Some(ws_num) = workspace_num_for_con_id(&tree, con_id) else {
        return;
    };
    // Portal-50.b.consumer (R12-Q11): per-workspace override wins
    // over the tag's default_layout. Load the workspaces.json file
    // fresh so operator edits via Portal-50's ✕ button take effect
    // immediately. Falls back to the tag default when no override
    // is set for this workspace.
    let override_layout = WorkspaceOverridesFile::load_default()
        .ok()
        .and_then(|f| f.layout_override(ws_num).map(str::to_string));
    let desired_string: String;
    let desired: &str = match override_layout.as_deref() {
        Some(layout) => {
            // Override exists — use it directly, skip tag lookup.
            desired_string = layout.to_string();
            &desired_string
        }
        None => {
            // No override — fall back to the owning tag's default.
            let store = match TagStore::load_default() {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(error = %e, "tag_layout tag-store load failed; skipping");
                    return;
                }
            };
            let Some(owning) = find_owning_tag(&store, ws_num) else {
                return;
            };
            // HYP-10.layout-bridge — tag-manifest `layout` wins over
            // TagStore `default_layout` when set + recognised. The
            // manifest's "mde" sentinel (compositor's native
            // algorithm) is intentionally treated as "no preference"
            // here — under sway it falls through to TagStore, then
            // through to "no command issued."
            let manifest_layout = crate::config::default_manifests_dir()
                .and_then(|d| crate::config::load_tag_manifests(&d).ok())
                .and_then(|ms| {
                    ms.iter()
                        .find(|m| m.name == owning.name)
                        .map(|m| m.layout.clone())
                })
                .filter(|l| is_recognised_layout(l));
            let layout = match manifest_layout {
                Some(l) => l,
                None => match owning.default_layout.clone() {
                    Some(l) => l,
                    None => return,
                },
            };
            desired_string = layout;
            &desired_string
        }
    };
    if !is_recognised_layout(desired) {
        tracing::debug!(workspace = ws_num, %desired, "tag_layout desired layout unrecognised; skipping");
        return;
    }
    // Only apply on the very first window in the workspace —
    // subsequent windows let the operator drive layout choice.
    let Some(window_count) = window_count_on_workspace(&tree, ws_num) else {
        return;
    };
    if window_count != 1 {
        return;
    }
    let current = current_layout(&tree, ws_num).unwrap_or_default();
    if current == desired {
        return;
    }
    let cmd = layout_command(desired);
    let source = if override_layout.is_some() {
        "workspace-override"
    } else {
        "tag-default"
    };
    match conn.run_command(&cmd).await {
        Ok(_) => tracing::debug!(workspace = ws_num, %desired, source, "tag_layout applied"),
        Err(e) => {
            tracing::warn!(workspace = ws_num, %desired, source, error = %e, "tag_layout command failed")
        }
    }
}

// ── Pure helpers ────────────────────────────────────────────────────────

/// True if `name` is one of the four sway layout names the design
/// lock recognises. Anything else is silently skipped to avoid
/// passing garbage through to swayipc.
#[must_use]
pub fn is_recognised_layout(name: &str) -> bool {
    matches!(name, "splith" | "splitv" | "tabbed" | "stacked")
}

/// Portal-50.b.consumer (R12-Q11): pure-function precedence
/// helper. Per-workspace override wins; tag default falls back
/// when override is None.
///
/// `None` from this function means "no opinion; let sway pick"
/// (no override + no tag default + no owning tag).
///
/// Exposed as `pub` so the precedence contract can be tested
/// without a sway connection.
#[must_use]
pub fn resolve_desired_layout<'a>(
    override_layout: Option<&'a str>,
    tag_default: Option<&'a str>,
) -> Option<&'a str> {
    override_layout.or(tag_default)
}

/// HYP-10.layout-bridge — precedence helper with the tag-manifest
/// layer added. Returns the layout to apply per:
///
/// 1. `override_layout` — per-workspace override (Portal-50.b).
/// 2. `manifest_layout` when set AND recognised (i.e. not the
///    "mde" sentinel which means "compositor's native").
/// 3. `tagstore_layout` — Portal-18.a legacy.
/// 4. `None` — no opinion; sway picks naturally.
///
/// Exposed as `pub` so the precedence contract can be tested
/// without a sway connection.
#[must_use]
pub fn resolve_desired_layout_with_manifest<'a>(
    override_layout: Option<&'a str>,
    manifest_layout: Option<&'a str>,
    tagstore_layout: Option<&'a str>,
) -> Option<&'a str> {
    if let Some(l) = override_layout {
        return Some(l);
    }
    if let Some(l) = manifest_layout {
        if is_recognised_layout(l) {
            return Some(l);
        }
    }
    tagstore_layout
}

/// Build the swayipc command string for `layout <name>`.
#[must_use]
pub fn layout_command(name: &str) -> String {
    format!("layout {name}")
}

/// Walk the sway tree to find the workspace number that contains
/// `con_id`. Returns `None` if the container isn't in any
/// workspace (e.g. scratchpad or floating before workspace
/// assignment).
fn workspace_num_for_con_id(node: &swayipc_async::Node, con_id: i64) -> Option<i32> {
    if node.node_type == swayipc_async::NodeType::Workspace {
        if walk_finds_con_id(node, con_id) {
            return node.num;
        }
    }
    for child in &node.nodes {
        if let Some(found) = workspace_num_for_con_id(child, con_id) {
            return Some(found);
        }
    }
    None
}

fn walk_finds_con_id(node: &swayipc_async::Node, target: i64) -> bool {
    if node.id == target {
        return true;
    }
    node.nodes.iter().any(|n| walk_finds_con_id(n, target))
        || node
            .floating_nodes
            .iter()
            .any(|n| walk_finds_con_id(n, target))
}

/// Count leaf `Con` windows on workspace `ws_num`. Returns `None`
/// if the workspace isn't found in the tree.
fn window_count_on_workspace(node: &swayipc_async::Node, ws_num: i32) -> Option<usize> {
    let ws_node = find_workspace_node(node, ws_num)?;
    Some(count_leaves(ws_node))
}

fn find_workspace_node<'a>(
    node: &'a swayipc_async::Node,
    ws_num: i32,
) -> Option<&'a swayipc_async::Node> {
    if node.node_type == swayipc_async::NodeType::Workspace && node.num == Some(ws_num) {
        return Some(node);
    }
    for child in &node.nodes {
        if let Some(found) = find_workspace_node(child, ws_num) {
            return Some(found);
        }
    }
    None
}

fn count_leaves(node: &swayipc_async::Node) -> usize {
    if node.node_type == swayipc_async::NodeType::Con && node.nodes.is_empty() {
        return 1;
    }
    node.nodes.iter().map(count_leaves).sum()
}

/// Read the workspace's current container layout (`splith` /
/// `splitv` / `tabbed` / `stacked`) from the tree. Returns
/// `None` if the workspace isn't found.
fn current_layout(node: &swayipc_async::Node, ws_num: i32) -> Option<String> {
    let ws_node = find_workspace_node(node, ws_num)?;
    Some(format!("{:?}", ws_node.layout).to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognised_layouts_lock_the_four_names() {
        assert!(is_recognised_layout("splith"));
        assert!(is_recognised_layout("splitv"));
        assert!(is_recognised_layout("tabbed"));
        assert!(is_recognised_layout("stacked"));
        assert!(!is_recognised_layout("Splith"));
        assert!(!is_recognised_layout("splith "));
        assert!(!is_recognised_layout(""));
        assert!(!is_recognised_layout("output"));
    }

    #[test]
    fn layout_command_round_trips() {
        assert_eq!(layout_command("splith"), "layout splith");
        assert_eq!(layout_command("tabbed"), "layout tabbed");
        // Garbage in / garbage out — the caller is expected to
        // gate via `is_recognised_layout`. Lock the contract so
        // an upstream bug surfaces in tests.
        assert_eq!(layout_command("garbage"), "layout garbage");
    }

    /// Portal-50.b.consumer: per-workspace override wins over the
    /// tag's default_layout. Mirrors the bench-acceptance:
    /// "with workspaces.json `{"1":{"layout_override":"tabbed"}}`
    /// + Dev `default_layout = "splith"` → ws1 with one window
    /// gets tabbed, not splith."
    #[test]
    fn resolve_desired_layout_override_wins() {
        assert_eq!(
            resolve_desired_layout(Some("tabbed"), Some("splith")),
            Some("tabbed")
        );
    }

    /// Tag default wins when no override is set.
    #[test]
    fn resolve_desired_layout_falls_back_to_tag_default() {
        assert_eq!(resolve_desired_layout(None, Some("splith")), Some("splith"));
    }

    /// No opinion when both sources are unset.
    #[test]
    fn resolve_desired_layout_none_when_neither_set() {
        assert_eq!(resolve_desired_layout(None, None), None);
    }

    /// Override-only (no tag default) still wins. This covers the
    /// case where the operator declined for a tagless workspace
    /// (though in practice Portal-50's spawn-gate requires the
    /// workspace be tag-owned; the override survives the tag
    /// being un-set later).
    #[test]
    fn resolve_desired_layout_override_only() {
        assert_eq!(resolve_desired_layout(Some("tabbed"), None), Some("tabbed"));
    }

    // ── HYP-10.layout-bridge — tag-manifest precedence chain ──

    /// Override beats everything, including manifest layout.
    #[test]
    fn resolve_with_manifest_override_wins_over_manifest() {
        let r =
            resolve_desired_layout_with_manifest(Some("tabbed"), Some("splith"), Some("splitv"));
        assert_eq!(r, Some("tabbed"));
    }

    /// Manifest with a recognised layout beats tagstore default.
    #[test]
    fn resolve_with_manifest_recognised_beats_tagstore() {
        let r = resolve_desired_layout_with_manifest(None, Some("splith"), Some("splitv"));
        assert_eq!(r, Some("splith"));
    }

    /// Manifest "mde" sentinel falls through to tagstore.
    #[test]
    fn resolve_with_manifest_mde_falls_through_to_tagstore() {
        let r = resolve_desired_layout_with_manifest(None, Some("mde"), Some("tabbed"));
        assert_eq!(r, Some("tabbed"));
    }

    /// Manifest unrecognised value falls through to tagstore.
    #[test]
    fn resolve_with_manifest_unrecognised_falls_through() {
        let r =
            resolve_desired_layout_with_manifest(None, Some("not-a-real-layout"), Some("splitv"));
        assert_eq!(r, Some("splitv"));
    }

    /// Empty manifest + empty tagstore + empty override → None.
    #[test]
    fn resolve_with_manifest_none_when_nothing_set() {
        let r = resolve_desired_layout_with_manifest(None, None, None);
        assert_eq!(r, None);
    }

    /// Manifest set to "mde", tagstore None → still None (the
    /// "mde" sentinel means no compositor-side override).
    #[test]
    fn resolve_with_manifest_mde_with_no_tagstore_returns_none() {
        let r = resolve_desired_layout_with_manifest(None, Some("mde"), None);
        assert_eq!(r, None);
    }
}
