//! Portal-48 (v6.0, R12-Q8 + R12-Q10) — auto-mark daemon.
//!
//! Subscribes to sway's `EventType::Window` and, on every
//! `WindowChange::New`, classifies the new window's `app_id` against
//! a static taxonomy table and runs `[con_id=N] mark --add <name>`
//! when a match exists. Five buckets:
//!
//!   * `editor`  — helix, code, vim, emacs, nvim, kakoune
//!   * `web`     — firefox, chromium, librewolf, qutebrowser, brave
//!   * `shell`   — foot, alacritty, kitty, wezterm, ghostty
//!   * `mail`    — thunderbird, geary, evolution, mutt
//!   * `chat`    — discord, element-desktop, signal-desktop,
//!                 telegram-desktop, slack
//!
//! Operator marks (anything already on `node.marks` at `window::new`
//! time) are preserved; the daemon only adds a taxonomy mark when
//! the new window has zero marks, so an operator pre-marking a
//! window via `swaymsg mark <foo>` before launch (rare but valid)
//! still wins.
//!
//! Marks are sway-session-ephemeral: not GFS-synced, not persisted
//! to disk. The downstream Portal-49 running-zone mark-pill render
//! reads them via `swaymsg get_tree` directly (lowest-latency,
//! no IPC).
//!
//! The cross-peer `dev.mackes.MDE.AutoMark.GetMarks()` zbus surface
//! from the original Portal-48 spec is deferred to a Portal-48.b
//! follow-on per CLAUDE.md §0.14 (newer-wins): Q20 + Q96 of the
//! 100-Q tightening survey lock the canonical IPC for MDE-internal
//! control on Mackes Bus, not D-Bus. The local auto-marking half
//! ships here so Portal-49 can render against real data; the
//! cross-peer surface lands on Bus once the broker is fully
//! wired (BUS-1.2+).

#![cfg(feature = "async-services")]

use std::time::Duration;

use futures_util::StreamExt as _;
use swayipc_async::{Connection, EventType};

use super::{ShutdownToken, Worker};

/// Backoff after a swayipc connect failure. Matches the
/// `workspace_namer` worker's cadence (Portal-41) for fleet-wide
/// reconnect lockstep.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(3);

/// Empty-state worker; all state lives on the stack inside `run`.
pub struct AutoMarkWorker;

impl AutoMarkWorker {
    /// Construct a fresh worker. No configuration — taxonomy is
    /// compile-time-locked.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for AutoMarkWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for AutoMarkWorker {
    fn name(&self) -> &'static str {
        "auto_mark"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            if shutdown.is_shutdown() {
                return Ok(());
            }
            let mut cmd_conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "auto_mark cmd-conn connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            let event_conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "auto_mark event-conn connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            let mut events = match event_conn.subscribe([EventType::Window]).await {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::debug!(error = %e, "auto_mark subscribe failed; backing off");
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
                                tracing::debug!(error = %e, "auto_mark event stream errored; reconnecting");
                                break;
                            }
                            None => {
                                tracing::debug!("auto_mark event stream ended; reconnecting");
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

/// Handle a `WindowChange::New` event. Two passes:
///
///   1. Taxonomy pass — if the compile-time `app_id` table matches
///      AND `container.marks` is empty, fire `[con_id=N] mark --add
///      <taxonomy>`.
///   2. HYP-AutoMark.sway-bridge — load `tag_manifest::load_all` +
///      `get_workspaces`, find the focused workspace, and apply
///      every mark from the matching manifest's `marks_default`
///      (comma-separated). Skipped when the window already has
///      operator marks at event time — operator-first wins.
///
/// Both passes respect the operator-first invariant: a window the
/// operator pre-marked via `swaymsg mark <foo>` before launch keeps
/// its mark set unchanged.
async fn handle_new_window(conn: &mut Connection, container: &swayipc_async::Node) {
    let con_id = container.id;

    // Pass 1: taxonomy mark from compile-time table.
    if let Some(taxonomy) = decide_mark(container.app_id.as_deref(), &container.marks) {
        let cmd = format!("[con_id={con_id}] mark --add {taxonomy}");
        match conn.run_command(&cmd).await {
            Ok(_) => tracing::debug!(
                con_id,
                app_id = ?container.app_id,
                %taxonomy,
                "auto_mark taxonomy applied",
            ),
            Err(e) => tracing::warn!(
                con_id,
                %taxonomy,
                error = %e,
                "auto_mark taxonomy command failed",
            ),
        }
    }

    // Pass 2: HYP-AutoMark.sway-bridge — tag-manifest marks_default.
    // The taxonomy pass above may have applied a mark, but operator
    // marks are still empty at the WindowChange::New snapshot we
    // captured in `container.marks`, so the operator-first guard
    // still uses `container.marks` (not the post-taxonomy state).
    if !container.marks.is_empty() {
        return;
    }
    let workspaces = match conn.get_workspaces().await {
        Ok(w) => w,
        Err(e) => {
            tracing::debug!(
                con_id,
                error = %e,
                "auto_mark get_workspaces failed; skipping marks_default pass",
            );
            return;
        }
    };
    let Some(focused) = workspaces.iter().find(|w| w.focused) else {
        return;
    };
    let manifests = match crate::config::tag_manifest::load_all(
        &crate::config::tag_manifest::default_manifests_dir().unwrap_or_default(),
    ) {
        Ok(ms) => ms,
        Err(e) => {
            tracing::debug!(
                con_id,
                error = %e,
                "auto_mark tag_manifest::load_all failed; skipping marks_default pass",
            );
            return;
        }
    };
    let marks = marks_default_for_workspace(&focused.name, &container.marks, Some(&manifests));
    for mark in &marks {
        let cmd = format!("[con_id={con_id}] mark --add {mark}");
        match conn.run_command(&cmd).await {
            Ok(_) => tracing::debug!(
                con_id,
                workspace = %focused.name,
                %mark,
                "auto_mark marks_default applied",
            ),
            Err(e) => tracing::warn!(
                con_id,
                workspace = %focused.name,
                %mark,
                error = %e,
                "auto_mark marks_default command failed",
            ),
        }
    }
}

// ── Pure helpers (testable without a sway connection) ───────────────────

/// Compile-time taxonomy table. Returns `Some(category)` if `app_id`
/// matches one of the five buckets, `None` otherwise. The mapping is
/// frozen at compile time per the Portal-48 design lock — any new
/// entries land via a new commit, not via config.
#[must_use]
pub fn taxonomy_for_app_id(app_id: &str) -> Option<&'static str> {
    match app_id {
        "helix" | "code" | "vim" | "emacs" | "nvim" | "kakoune" => Some("editor"),
        "firefox" | "chromium" | "librewolf" | "qutebrowser" | "brave" => Some("web"),
        "foot" | "alacritty" | "kitty" | "wezterm" | "ghostty" => Some("shell"),
        "thunderbird" | "geary" | "evolution" | "mutt" => Some("mail"),
        "discord" | "element-desktop" | "signal-desktop" | "telegram-desktop" | "slack" => {
            Some("chat")
        }
        _ => None,
    }
}

/// Decide whether to apply a taxonomy mark. Returns `Some(taxonomy)`
/// if `app_id` matches the taxonomy AND `existing_marks` is empty;
/// `None` otherwise (no app_id, unknown app, or marks already
/// present). Operator marks always win — the daemon never overwrites.
#[must_use]
pub fn decide_mark(app_id: Option<&str>, existing_marks: &[String]) -> Option<&'static str> {
    if !existing_marks.is_empty() {
        return None;
    }
    let app_id = app_id?;
    if app_id.is_empty() {
        return None;
    }
    taxonomy_for_app_id(app_id)
}

/// HYP-AutoMark.sway-bridge — resolve the per-workspace
/// `marks_default` list for a new window. Returns the comma-split,
/// trimmed, filtered marks that should be applied, OR an empty Vec
/// when:
///
///   - `existing_marks` is non-empty (operator pre-mark wins);
///   - `manifests` is `None` (no snapshot available, e.g. load
///     failure or pre-mount boot);
///   - no manifest matches `workspace_name`;
///   - the matching manifest's `marks_default` is empty/whitespace.
///
/// Pure function — no swayipc connection, no filesystem access.
/// Returns `Vec<String>` (rather than `Vec<&str>`) so the worker
/// can issue swayipc commands without borrowing from the manifest
/// snapshot across `await` points.
#[must_use]
pub fn marks_default_for_workspace(
    workspace_name: &str,
    existing_marks: &[String],
    manifests: Option<&[crate::config::TagManifest]>,
) -> Vec<String> {
    if !existing_marks.is_empty() {
        return Vec::new();
    }
    let Some(ms) = manifests else {
        return Vec::new();
    };
    let Some(m) = ms.iter().find(|m| m.name == workspace_name) else {
        return Vec::new();
    };
    m.marks_default
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Five canonical app_ids — one per bucket — round-trip
    /// through `taxonomy_for_app_id` to their bucket name.
    #[test]
    fn taxonomy_table_matches_canonical_app_ids() {
        assert_eq!(taxonomy_for_app_id("firefox"), Some("web"));
        assert_eq!(taxonomy_for_app_id("helix"), Some("editor"));
        assert_eq!(taxonomy_for_app_id("foot"), Some("shell"));
        assert_eq!(taxonomy_for_app_id("thunderbird"), Some("mail"));
        assert_eq!(taxonomy_for_app_id("discord"), Some("chat"));
    }

    /// All 25 taxonomy entries land in the table.
    #[test]
    fn taxonomy_table_covers_all_buckets() {
        // editor (6)
        for app in ["helix", "code", "vim", "emacs", "nvim", "kakoune"] {
            assert_eq!(taxonomy_for_app_id(app), Some("editor"), "{app}");
        }
        // web (5)
        for app in ["firefox", "chromium", "librewolf", "qutebrowser", "brave"] {
            assert_eq!(taxonomy_for_app_id(app), Some("web"), "{app}");
        }
        // shell (5)
        for app in ["foot", "alacritty", "kitty", "wezterm", "ghostty"] {
            assert_eq!(taxonomy_for_app_id(app), Some("shell"), "{app}");
        }
        // mail (4)
        for app in ["thunderbird", "geary", "evolution", "mutt"] {
            assert_eq!(taxonomy_for_app_id(app), Some("mail"), "{app}");
        }
        // chat (5)
        for app in [
            "discord",
            "element-desktop",
            "signal-desktop",
            "telegram-desktop",
            "slack",
        ] {
            assert_eq!(taxonomy_for_app_id(app), Some("chat"), "{app}");
        }
    }

    /// Unknown app_ids return None — no auto-mark, no error.
    #[test]
    fn unknown_app_ids_are_passthrough() {
        assert_eq!(taxonomy_for_app_id("unknown-app"), None);
        assert_eq!(taxonomy_for_app_id("org.mozilla.Firefox"), None); // exact-match only
        assert_eq!(taxonomy_for_app_id("FIREFOX"), None); // case-sensitive
        assert_eq!(taxonomy_for_app_id(""), None);
    }

    /// Operator marks block auto-marking — if any existing mark is
    /// present on the new window's container, the daemon skips.
    #[test]
    fn existing_marks_block_auto_mark() {
        // Operator already marked firefox with a custom name.
        let existing = vec!["work".to_string()];
        assert_eq!(decide_mark(Some("firefox"), &existing), None);
    }

    /// Empty marks + matching taxonomy = mark gets applied.
    #[test]
    fn empty_marks_with_matching_taxonomy_applies() {
        let no_marks: Vec<String> = Vec::new();
        assert_eq!(decide_mark(Some("firefox"), &no_marks), Some("web"));
        assert_eq!(decide_mark(Some("foot"), &no_marks), Some("shell"));
    }

    /// Empty app_id never marks — covers the xwayland case where
    /// app_id is None.
    #[test]
    fn empty_or_missing_app_id_skips() {
        let no_marks: Vec<String> = Vec::new();
        assert_eq!(decide_mark(None, &no_marks), None);
        assert_eq!(decide_mark(Some(""), &no_marks), None);
    }

    /// Unknown app_id + empty marks = still skip. The daemon doesn't
    /// invent marks for apps outside the taxonomy.
    #[test]
    fn unknown_app_with_empty_marks_skips() {
        let no_marks: Vec<String> = Vec::new();
        assert_eq!(decide_mark(Some("custom-app"), &no_marks), None);
    }

    // ── HYP-AutoMark.sway-bridge — marks_default tests ───────────────────

    /// Build a small manifest with a `marks_default` string for
    /// testing. Other fields use the schema defaults.
    fn manifest_with_marks(name: &str, marks_default: &str) -> crate::config::TagManifest {
        crate::config::TagManifest {
            name: name.to_string(),
            output: None,
            apps: Vec::new(),
            layout: "mde".to_string(),
            marks_default: marks_default.to_string(),
            border_color: None,
            autostart: false,
        }
    }

    /// Happy path: matching manifest with two marks, no operator
    /// pre-marks, returns both marks.
    #[test]
    fn marks_default_happy_path_two_marks() {
        let no_marks: Vec<String> = Vec::new();
        let ms = vec![manifest_with_marks("voip", "priority,call")];
        let result = marks_default_for_workspace("voip", &no_marks, Some(&ms));
        assert_eq!(result, vec!["priority".to_string(), "call".to_string()]);
    }

    /// Whitespace + empty entries get trimmed/filtered.
    #[test]
    fn marks_default_trims_whitespace_and_filters_empty() {
        let no_marks: Vec<String> = Vec::new();
        let ms = vec![manifest_with_marks("dev", " one , , two ,three,")];
        let result = marks_default_for_workspace("dev", &no_marks, Some(&ms));
        assert_eq!(
            result,
            vec!["one".to_string(), "two".to_string(), "three".to_string()]
        );
    }

    /// Operator pre-marks block tag-marks just like they block
    /// taxonomy marks — operator-first invariant.
    #[test]
    fn marks_default_operator_marks_block() {
        let existing = vec!["work".to_string()];
        let ms = vec![manifest_with_marks("voip", "priority,call")];
        let result = marks_default_for_workspace("voip", &existing, Some(&ms));
        assert!(result.is_empty());
    }

    /// No manifest matches the workspace name → empty.
    #[test]
    fn marks_default_no_matching_manifest_returns_empty() {
        let no_marks: Vec<String> = Vec::new();
        let ms = vec![manifest_with_marks("voip", "priority")];
        let result = marks_default_for_workspace("dev", &no_marks, Some(&ms));
        assert!(result.is_empty());
    }

    /// Empty manifests slice → empty.
    #[test]
    fn marks_default_empty_manifest_slice_returns_empty() {
        let no_marks: Vec<String> = Vec::new();
        let ms: Vec<crate::config::TagManifest> = Vec::new();
        let result = marks_default_for_workspace("voip", &no_marks, Some(&ms));
        assert!(result.is_empty());
    }

    /// `None` manifests snapshot (load failure / pre-mount boot) →
    /// empty. Pass-through behavior matches the other sway-bridge
    /// workers' `None`-snapshot path.
    #[test]
    fn marks_default_none_manifests_returns_empty() {
        let no_marks: Vec<String> = Vec::new();
        let result = marks_default_for_workspace("voip", &no_marks, None);
        assert!(result.is_empty());
    }

    /// Empty marks_default string returns empty (no marks to apply).
    #[test]
    fn marks_default_empty_string_returns_empty() {
        let no_marks: Vec<String> = Vec::new();
        let ms = vec![manifest_with_marks("voip", "")];
        let result = marks_default_for_workspace("voip", &no_marks, Some(&ms));
        assert!(result.is_empty());
    }

    /// Whitespace-only marks_default returns empty.
    #[test]
    fn marks_default_whitespace_only_returns_empty() {
        let no_marks: Vec<String> = Vec::new();
        let ms = vec![manifest_with_marks("voip", "   , , ,  ")];
        let result = marks_default_for_workspace("voip", &no_marks, Some(&ms));
        assert!(result.is_empty());
    }
}
