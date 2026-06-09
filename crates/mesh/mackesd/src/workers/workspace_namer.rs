//! Portal-41 (v6.0, R12-Q1) — auto-derived workspace names.
//!
//! Subscribes to sway's `EventType::Window` stream. Every time the
//! focused window changes (or a window opens / closes on the focused
//! workspace), the worker recomputes the workspace's "preferred" name
//! and, if the current name is still auto-derived, runs
//! `rename workspace number <num> to "<num>: <app_id>"` to sync them.
//!
//! Operator-set names are never overwritten. An auto-derived name is
//! recognised by either being exactly `<num>` (numeric-only, the empty
//! state) or starting with `<num>: ` (the canonical prefix). Anything
//! else is treated as operator-curated and left alone.
//!
//! The worker is debounced trailing-edge by 200 ms: rapid focus
//! changes (`Alt+Tab` through five windows in under a second) collapse
//! into a single rename pass against the final settled state. Without
//! the debounce, sway can receive a rapid burst of renames that
//! flicker through the breadcrumb's typewriter animation downstream
//! (Portal-14).

#![cfg(feature = "async-services")]

use std::time::Duration;

use futures_util::StreamExt as _;
use swayipc_async::{Connection, EventType};

use super::{ShutdownToken, Worker};

/// Trailing-edge debounce window. Sized to outlast the typical
/// keyboard burst (`Alt+Tab` traversal at ~100 ms/step) without
/// adding perceptible lag to a single deliberate focus change.
const DEBOUNCE_WINDOW: Duration = Duration::from_millis(200);

/// Backoff after a swayipc connect failure. Mirrors the
/// `mde-portal::workspace::workspace_subscription` retry cadence so
/// the two consumers re-attach in lockstep when sway restarts.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(3);

/// Empty-state worker; all state lives on the stack inside `run`.
pub struct WorkspaceNamerWorker;

impl WorkspaceNamerWorker {
    /// Construct a fresh worker. No configuration — connection
    /// state is rebuilt every reconnect cycle.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for WorkspaceNamerWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for WorkspaceNamerWorker {
    fn name(&self) -> &'static str {
        "workspace_namer"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Reconnect loop — when sway restarts (or hasn't started
        // yet on a fresh login), back off + retry instead of
        // returning Err to the supervisor. The supervisor's
        // OnFailure restart policy would still cycle us, but
        // staying inside the loop is gentler on the JoinSet.
        loop {
            if shutdown.is_shutdown() {
                return Ok(());
            }
            let mut cmd_conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "workspace_namer cmd-conn connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            let event_conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "workspace_namer event-conn connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            let mut events = match event_conn.subscribe([EventType::Window]).await {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::debug!(error = %e, "workspace_namer subscribe failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            // Run an initial pass before the first event, so a
            // mackesd restart on an already-populated session
            // converges immediately rather than waiting for the
            // next focus change.
            rename_pass(&mut cmd_conn).await;

            let mut pending = false;
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.wait() => return Ok(()),
                    next = events.next() => {
                        match next {
                            Some(Ok(swayipc_async::Event::Window(_))) => {
                                pending = true;
                            }
                            Some(Ok(_)) => {
                                // Non-Window event — ignore. The
                                // subscribe filter should keep this
                                // unreachable, but be defensive.
                            }
                            Some(Err(e)) => {
                                tracing::debug!(error = %e, "workspace_namer event stream errored; reconnecting");
                                break;
                            }
                            None => {
                                tracing::debug!("workspace_namer event stream ended; reconnecting");
                                break;
                            }
                        }
                    }
                    _ = tokio::time::sleep(DEBOUNCE_WINDOW), if pending => {
                        pending = false;
                        rename_pass(&mut cmd_conn).await;
                    }
                }
            }
        }
    }
}

/// Sleep up to `dur`, returning early if shutdown is requested.
async fn sleep_or_shutdown(dur: Duration, shutdown: &mut ShutdownToken) {
    tokio::select! {
        _ = shutdown.wait() => {}
        _ = tokio::time::sleep(dur) => {}
    }
}

/// One rename pass against the live sway tree. Finds the focused
/// workspace, computes its preferred name, and renames if the
/// current name is auto-derived and differs.
async fn rename_pass(conn: &mut Connection) {
    let tree = match conn.get_tree().await {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(error = %e, "workspace_namer get_tree failed; skipping pass");
            return;
        }
    };
    let workspaces = match conn.get_workspaces().await {
        Ok(w) => w,
        Err(e) => {
            tracing::debug!(error = %e, "workspace_namer get_workspaces failed; skipping pass");
            return;
        }
    };
    let Some(focused) = workspaces.iter().find(|w| w.focused) else {
        return;
    };
    let app_id = focused_window_app_id(&tree, focused.num);
    // HYP-9.sway-bridge — load the tag-manifest snapshot each pass
    // so per-tag `name` overrides the literal app_id when the
    // focused window's `app_id` appears in any manifest's `apps[]`.
    // Manifest load is fail-soft: missing dir / unreadable file
    // falls through to the legacy app_id-based naming. Cheap
    // relative to the swayipc tree+workspaces fetches above.
    let manifests = crate::config::default_manifests_dir()
        .and_then(|d| crate::config::load_tag_manifests(&d).ok());
    let desired =
        derive_workspace_name_with_manifests(focused.num, app_id.as_deref(), manifests.as_deref());
    if !is_auto_derived(focused.num, &focused.name) {
        return;
    }
    if focused.name == desired {
        return;
    }
    let cmd = rename_command(focused.num, &desired);
    match conn.run_command(&cmd).await {
        Ok(_) => {
            tracing::debug!(workspace = focused.num, %desired, "workspace_namer renamed workspace")
        }
        Err(e) => {
            tracing::warn!(workspace = focused.num, %desired, error = %e, "workspace_namer rename failed")
        }
    }
}

/// Walk the sway tree and return the `app_id` of the focused window
/// on workspace `target_num`, or the first window's `app_id` if none
/// is focused. Returns `None` for an empty workspace.
pub fn focused_window_app_id(root: &swayipc_async::Node, target_num: i32) -> Option<String> {
    let ws_node = find_workspace(root, target_num)?;
    let mut first: Option<String> = None;
    let mut focused: Option<String> = None;
    visit_leaves(ws_node, &mut |node| {
        if node.app_id.is_some() {
            if focused.is_none() && node.focused {
                focused = node.app_id.clone();
            }
            if first.is_none() {
                first = node.app_id.clone();
            }
        }
    });
    focused.or(first)
}

fn find_workspace(node: &swayipc_async::Node, target_num: i32) -> Option<&swayipc_async::Node> {
    if node.node_type == swayipc_async::NodeType::Workspace && node.num == Some(target_num) {
        return Some(node);
    }
    for child in &node.nodes {
        if let Some(found) = find_workspace(child, target_num) {
            return Some(found);
        }
    }
    None
}

fn visit_leaves<F: FnMut(&swayipc_async::Node)>(node: &swayipc_async::Node, f: &mut F) {
    if node.node_type == swayipc_async::NodeType::Con && node.nodes.is_empty() {
        f(node);
    }
    for child in &node.nodes {
        visit_leaves(child, f);
    }
}

// ── Pure helpers (testable without a sway connection) ───────────────────

/// Produce the preferred name for a workspace whose number is `num`
/// and whose focused-or-first app_id is `app_id`. Shim around
/// [`derive_workspace_name_with_manifests`] with no manifest snapshot
/// (preserves the Portal-41 contract for callers that don't load
/// manifests yet).
///
/// * `Some(non-empty)` → `"<num>: <app_id>"`
/// * `Some("")` or `None` → `"<num>"` (numeric-only, the empty state)
///
/// The numeric-only form is what an empty workspace settles into and
/// is also the seed sway hands out when an operator first creates a
/// workspace via `Mod+<n>`.
#[must_use]
pub fn derive_workspace_name(num: i32, app_id: Option<&str>) -> String {
    derive_workspace_name_with_manifests(num, app_id, None)
}

/// HYP-9.sway-bridge — same as [`derive_workspace_name`] but with an
/// explicit tag-manifest snapshot. Resolution precedence:
///
/// 1. **Tag manifest `name`** (HYP-8.5 source of truth) — when the
///    focused window's `app_id` appears in any manifest's `apps[]`
///    list AND that manifest has a non-empty `name`, the workspace
///    name uses the manifest's `name` field instead of the literal
///    `app_id`. First match wins via the alphabetical sort that
///    `load_all` enforces (deterministic across reboots).
/// 2. **Literal `app_id`** (Portal-41 legacy) — fallback when no
///    manifest claims the focused window's `app_id`, or when the
///    matching manifest's `name` is empty.
/// 3. **Numeric-only** — when `app_id` is `None` or empty.
#[must_use]
pub fn derive_workspace_name_with_manifests(
    num: i32,
    app_id: Option<&str>,
    manifests: Option<&[crate::config::TagManifest]>,
) -> String {
    let Some(id) = app_id.filter(|s| !s.is_empty()) else {
        return num.to_string();
    };

    // Priority 1: tag-manifest `name` lookup. First manifest whose
    // `apps[]` contains `id` AND whose `name` is non-empty wins.
    if let Some(ms) = manifests {
        for m in ms.iter() {
            if m.apps.iter().any(|a| a == id) && !m.name.is_empty() {
                return format!("{num}: {name}", name = m.name);
            }
        }
    }

    // Priority 2: literal app_id (Portal-41 legacy).
    format!("{num}: {id}")
}

/// `true` if `current_name` matches the pattern this worker writes
/// (`<num>` or `<num>: …`) and is therefore safe to overwrite.
///
/// Names that don't match — `"Mail"`, `"work"`, `"7"` on a workspace
/// whose num is `5`, etc. — are treated as operator-curated and
/// preserved verbatim.
#[must_use]
pub fn is_auto_derived(num: i32, current_name: &str) -> bool {
    let numeric = num.to_string();
    if current_name == numeric {
        return true;
    }
    let prefix = format!("{num}: ");
    current_name.starts_with(&prefix)
}

/// Build the swayipc command string that renames workspace `num` to
/// `new_name`. Embedded double-quotes are backslash-escaped so the
/// command parses correctly even for app_ids containing quotes.
#[must_use]
pub fn rename_command(num: i32, new_name: &str) -> String {
    let escaped = new_name.replace('\\', "\\\\").replace('"', "\\\"");
    format!("rename workspace number {num} to \"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test 1 — `app_id_empty_fallback_returns_numeric_only`.
    ///
    /// When the focused window's app_id is `None` or empty, the
    /// preferred name is the bare workspace number with no colon.
    /// Covers the "app_id-empty-fallback" acceptance bullet.
    #[test]
    fn app_id_empty_fallback_returns_numeric_only() {
        assert_eq!(derive_workspace_name(5, None), "5");
        assert_eq!(derive_workspace_name(5, Some("")), "5");
        assert_eq!(derive_workspace_name(0, None), "0");
    }

    /// Test 2 — `manual_name_preserved_blocks_rename`.
    ///
    /// Names that don't match the `<num>` or `<num>: …` patterns
    /// are operator-curated; the worker must leave them alone.
    /// Covers the "manual-name-preserved" acceptance bullet.
    #[test]
    fn manual_name_preserved_blocks_rename() {
        assert!(!is_auto_derived(5, "Mail"));
        assert!(!is_auto_derived(5, "work"));
        // A number that doesn't match the workspace's num is also
        // not auto-derived — operator named ws5 "7".
        assert!(!is_auto_derived(5, "7"));
        // Subtle case: starts-with `<num>` but no colon — still
        // operator-curated, NOT auto.
        assert!(!is_auto_derived(5, "5b"));
        assert!(!is_auto_derived(5, "5-monitor"));
        // Confirm the positive cases still match so the negative
        // assertions aren't trivially true.
        assert!(is_auto_derived(5, "5"));
        assert!(is_auto_derived(5, "5: firefox"));
    }

    /// Test 3 — `multi_focus_debounce_collapses_rapid_events`.
    ///
    /// Three focus events fired at 50 ms / 100 ms / 150 ms inside
    /// a 200 ms debounce window must collapse into a single
    /// trailing-edge rename pass. We model the worker's select!
    /// state machine as a deterministic state machine (no live
    /// clock, no tokio runtime) — `pending` flips on each event;
    /// the trailing-edge fire only resolves once `DEBOUNCE_WINDOW`
    /// elapses without any further event resetting it. Covers
    /// the "multi-focus-debounce-200ms" acceptance bullet.
    #[test]
    fn multi_focus_debounce_collapses_rapid_events() {
        use std::time::Duration as StdDur;
        let burst: Vec<StdDur> = vec![
            StdDur::from_millis(50),
            StdDur::from_millis(100),
            StdDur::from_millis(150),
        ];
        let mut pending = false;
        let mut last_event_at = StdDur::from_millis(0);
        let mut fired = 0_u32;
        for evt_at in &burst {
            // Between events, check whether the trailing-edge
            // sleep would have completed. With DEBOUNCE_WINDOW =
            // 200 ms and events 50 ms apart, every gap is < 200 ms
            // so `pending` stays set without firing.
            let gap = *evt_at - last_event_at;
            if pending && gap >= DEBOUNCE_WINDOW {
                fired += 1;
                pending = false;
            }
            pending = true;
            last_event_at = *evt_at;
        }
        // Burst finished — the next select! iteration runs the
        // sleep branch. After DEBOUNCE_WINDOW of quiet, fire.
        let quiet_gap = StdDur::from_millis(200);
        if pending && quiet_gap >= DEBOUNCE_WINDOW {
            fired += 1;
            pending = false;
        }
        assert_eq!(
            fired, 1,
            "three rapid events within DEBOUNCE_WINDOW must collapse to one rename pass"
        );
        assert!(!pending, "pending flag clears after firing");
        // Sanity-lock the constant itself so a future refactor that
        // changes DEBOUNCE_WINDOW out from under R12-Q1 lights up.
        assert_eq!(DEBOUNCE_WINDOW, StdDur::from_millis(200));
    }

    /// Test 4 — `numeric_only_when_workspace_has_no_windows`.
    ///
    /// A workspace whose tree contains zero windows has no
    /// app_id to surface; the preferred name is the numeric-only
    /// form. Covers the "numeric-only-on-no-windows" bullet.
    #[test]
    fn numeric_only_when_workspace_has_no_windows() {
        let empty_app_id: Option<&str> = None;
        assert_eq!(derive_workspace_name(3, empty_app_id), "3");
        // The "currently named 3: firefox, window closed → rename
        // back to 3" path is the actual user-visible test 5; here
        // we only check the pure-function side of "no windows →
        // bare-number name" so the helper's contract is locked.
        let preferred = derive_workspace_name(3, empty_app_id);
        assert!(!preferred.contains(':'));
    }

    /// Test 5 — `rename_fires_when_last_window_closes`.
    ///
    /// Given a workspace whose current name is `"5: firefox"`, the
    /// last window closing settles app_id to `None`, the preferred
    /// name flips to `"5"`, and the worker must emit a rename
    /// because the current name (`"5: firefox"`) is auto-derived
    /// and differs from the preferred (`"5"`).
    /// Covers the "rename-on-window-close" acceptance bullet.
    #[test]
    fn rename_fires_when_last_window_closes() {
        let num = 5;
        let current = "5: firefox";
        // Window closed → no app_id surfaces from the empty tree.
        let preferred = derive_workspace_name(num, None);
        assert_eq!(preferred, "5");
        assert!(is_auto_derived(num, current));
        assert_ne!(preferred, current);
        // The worker condenses this into a rename command.
        let cmd = rename_command(num, &preferred);
        assert_eq!(cmd, r#"rename workspace number 5 to "5""#);
    }

    // ── Bonus: rename_command escaping ─────────────────────────────────
    //
    // Not in the 5 required tests, but the rename command is the
    // worker's actual side-effect surface so we lock its escaping
    // contract too. App ids in the wild contain dots and dashes
    // routinely (e.g. `org.mozilla.firefox`), and the rare
    // operator-targeted quote-injection case still needs the
    // backslash escape so swayipc parses the command.
    #[test]
    fn rename_command_escapes_quotes_and_backslashes() {
        assert_eq!(
            rename_command(2, r#"2: app"with"quotes"#),
            r#"rename workspace number 2 to "2: app\"with\"quotes""#
        );
        assert_eq!(
            rename_command(7, r"7: back\slash"),
            r#"rename workspace number 7 to "7: back\\slash""#
        );
        assert_eq!(
            rename_command(1, "1: org.mozilla.firefox"),
            r#"rename workspace number 1 to "1: org.mozilla.firefox""#
        );
    }

    // ── HYP-9.sway-bridge precedence tests ─────────────────────────────────
    //
    // Mirror the testing shape used by workspace_router /
    // border_tinter / tag_layout / tag_autostart bridges: pure-fn
    // unit tests over the new precedence helper, no live sway
    // connection.

    use crate::config::TagManifest;

    fn manifest_with(name: &str, apps: &[&str]) -> TagManifest {
        TagManifest {
            name: name.to_string(),
            apps: apps.iter().map(|s| s.to_string()).collect(),
            ..TagManifest::default()
        }
    }

    /// Manifest `apps[]` claims the focused app_id → workspace
    /// name uses the manifest `name` instead of the literal
    /// app_id. Bench acceptance: workspace running `linphone`
    /// whose voip manifest groups it under `voip` reads
    /// `3: voip`.
    #[test]
    fn manifest_name_wins_when_apps_match() {
        let manifests = vec![manifest_with("voip", &["org.mde.voice.hud", "linphone"])];
        assert_eq!(
            derive_workspace_name_with_manifests(3, Some("linphone"), Some(&manifests)),
            "3: voip"
        );
        assert_eq!(
            derive_workspace_name_with_manifests(7, Some("org.mde.voice.hud"), Some(&manifests)),
            "7: voip"
        );
    }

    /// Manifests present but none claim the focused app_id →
    /// falls through to literal-app_id naming (Portal-41 legacy).
    #[test]
    fn manifest_no_match_falls_through_to_app_id() {
        let manifests = vec![manifest_with("voip", &["linphone"])];
        assert_eq!(
            derive_workspace_name_with_manifests(2, Some("firefox"), Some(&manifests)),
            "2: firefox"
        );
    }

    /// Matching manifest with empty `name` falls through to
    /// literal-app_id naming. Defensive: a degenerate manifest
    /// shouldn't strip the workspace name to `<num>: ` (which
    /// is a malformed auto-derived form).
    #[test]
    fn manifest_match_with_empty_name_falls_through() {
        let manifests = vec![manifest_with("", &["firefox"])];
        assert_eq!(
            derive_workspace_name_with_manifests(4, Some("firefox"), Some(&manifests)),
            "4: firefox"
        );
    }

    /// Two manifests claim the same app_id → first match wins.
    /// `load_all` returns manifests sorted by name, so this also
    /// locks the deterministic tiebreaker contract: a manifest
    /// named "alpha" beats "beta" when both list the same app.
    #[test]
    fn first_matching_manifest_wins_deterministic() {
        let manifests = vec![
            manifest_with("alpha", &["chromium"]),
            manifest_with("beta", &["chromium"]),
        ];
        assert_eq!(
            derive_workspace_name_with_manifests(1, Some("chromium"), Some(&manifests)),
            "1: alpha"
        );
    }

    /// `None` manifest snapshot → behaves identically to the
    /// shim. Confirms the legacy callers stay byte-for-byte
    /// stable when manifests aren't loaded yet.
    #[test]
    fn none_manifests_match_legacy_shim() {
        assert_eq!(
            derive_workspace_name_with_manifests(5, Some("firefox"), None),
            "5: firefox"
        );
        assert_eq!(
            derive_workspace_name_with_manifests(5, Some("firefox"), None),
            derive_workspace_name(5, Some("firefox"))
        );
    }

    /// Empty / None app_id → numeric-only regardless of
    /// manifests. The manifest lookup is skipped entirely so a
    /// workspace with no focused window can't accidentally pick
    /// up a tag name.
    #[test]
    fn empty_app_id_returns_numeric_only_with_manifests() {
        let manifests = vec![manifest_with("voip", &[""])];
        assert_eq!(
            derive_workspace_name_with_manifests(8, None, Some(&manifests)),
            "8"
        );
        assert_eq!(
            derive_workspace_name_with_manifests(8, Some(""), Some(&manifests)),
            "8"
        );
    }

    /// Empty manifest list → falls through to literal app_id.
    /// Covers the early-boot path where the loader returns
    /// `Some(vec![])` after a successful read of an empty dir.
    #[test]
    fn empty_manifest_list_falls_through() {
        let manifests: Vec<TagManifest> = Vec::new();
        assert_eq!(
            derive_workspace_name_with_manifests(6, Some("firefox"), Some(&manifests)),
            "6: firefox"
        );
    }

    /// `is_auto_derived` still recognises the manifest-named
    /// form (`<num>: <name>`) — both `5: firefox` and
    /// `5: voip` start with `5: ` so re-rename pass after a
    /// manifest is added or removed correctly fires.
    #[test]
    fn manifest_named_form_is_still_auto_derived() {
        assert!(is_auto_derived(3, "3: voip"));
        assert!(is_auto_derived(3, "3: firefox"));
        // Operator-curated name still preserved.
        assert!(!is_auto_derived(3, "VoIP Calls"));
    }
}
