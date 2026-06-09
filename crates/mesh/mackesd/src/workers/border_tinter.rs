//! Portal-56 (v6.0, R12-Q21) — per-workspace focused-border tinting.
//!
//! Subscribes to sway's `EventType::Workspace`. On every
//! `WorkspaceChange::Focus`, the worker looks up the newly-focused
//! workspace's owning tag (Portal-18.a schema) + reads
//! `tag.group_color`. It then fires swayipc
//!
//!   `client.focused <color> <color> #f4f4f4 <color> <color>`
//!
//! (border, background, text, indicator, child_border) to tint the
//! focused-window border to the tag's color. Tagless workspaces
//! fall back to the platform default Carbon blue (`#2b9af3`,
//! matching `data/sway/config:60`).
//!
//! Unfocused / urgent / placeholder colors are NOT touched — only
//! the focused state is recolored. Operator-configured per-workspace
//! UNFOCUSED colors stay as they are.

#![cfg(feature = "async-services")]

use std::time::Duration;

use futures_util::StreamExt as _;
use mackes_mesh_types::TagStore;
use swayipc_async::{Connection, EventType};

use super::workspace_router::find_owning_tag;
use super::{ShutdownToken, Worker};

const RECONNECT_BACKOFF: Duration = Duration::from_secs(3);

/// Platform fallback focused-border color when a workspace has no
/// owning tag or its tag has no `group_color`. Matches the Carbon
/// blue locked in `data/sway/config:60` `client.focused`.
pub const DEFAULT_FOCUSED_COLOR: &str = "#2b9af3";

/// Fixed text/foreground color in the swayipc `client.focused`
/// command — never tag-tinted. Off-white for readability against
/// any owning-tag color.
pub const FOCUSED_TEXT_COLOR: &str = "#f4f4f4";

/// Empty-state worker — tag store reloads per-event.
pub struct BorderTinterWorker;

impl BorderTinterWorker {
    /// Construct a fresh worker.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for BorderTinterWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for BorderTinterWorker {
    fn name(&self) -> &'static str {
        "border_tinter"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            if shutdown.is_shutdown() {
                return Ok(());
            }
            let mut cmd_conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "border_tinter cmd-conn connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            let event_conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "border_tinter event-conn connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            let mut events = match event_conn.subscribe([EventType::Workspace]).await {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::debug!(error = %e, "border_tinter subscribe failed; backing off");
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
                                if ws_ev.change == swayipc_async::WorkspaceChange::Focus {
                                    if let Some(node) = ws_ev.current.as_ref() {
                                        if let Some(num) = node.num {
                                            handle_focus(&mut cmd_conn, num).await;
                                        }
                                    }
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                tracing::debug!(error = %e, "border_tinter event stream errored; reconnecting");
                                break;
                            }
                            None => {
                                tracing::debug!("border_tinter event stream ended; reconnecting");
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

async fn handle_focus(conn: &mut Connection, num: i32) {
    let store = match TagStore::load_default() {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(error = %e, "border_tinter tag-store load failed; skipping focus event");
            return;
        }
    };
    // HYP-22.sway-bridge — also load the tag-manifest snapshot
    // each focus event so the per-tag `border_color` field can
    // override the TagStore color. Manifest load is fail-soft: a
    // missing dir or unreadable file just falls through to the
    // legacy TagStore path. Cheap relative to the swayipc
    // round-trip that follows.
    let manifests = crate::config::default_manifests_dir()
        .and_then(|d| crate::config::load_tag_manifests(&d).ok());
    let color = color_for_workspace_with_manifests(&store, num, manifests.as_deref());
    let cmd = client_focused_command(&color);
    match conn.run_command(&cmd).await {
        Ok(_) => tracing::debug!(workspace = num, %color, "border_tinter applied"),
        Err(e) => {
            tracing::warn!(workspace = num, %color, error = %e, "border_tinter command failed")
        }
    }
}

// ── Pure helpers ────────────────────────────────────────────────────────

/// Resolve the focused-border color for workspace `ws_num`. Returns
/// the first non-empty source in this precedence chain:
///
/// 1. **Tag manifest `border_color`** (HYP-8.5 source of truth) —
///    `~/.config/mde/tags/<name>.toml`. The compositor-side
///    visual policy lives here per the simplification re-lock;
///    when set, it wins over the legacy TagStore color.
/// 2. **TagStore `group_color`** (Portal-18.a legacy path) —
///    `~/.local/share/mde/tags.json`. Stays as fallback for
///    operators who configured tags before HYP-8.5 landed +
///    haven't migrated their colors yet.
/// 3. **Platform default** ([`DEFAULT_FOCUSED_COLOR`]) — Carbon
///    blue from `data/sway/config:60`.
///
/// `live_manifests` is the watcher's snapshot — usually the
/// per-tick result of `tag_manifest::load_all`. None = skip the
/// manifest layer (e.g. early-boot before tag_manifest_watcher
/// has primed); fall through to TagStore.
#[must_use]
pub fn color_for_workspace(store: &TagStore, ws_num: i32) -> String {
    color_for_workspace_with_manifests(store, ws_num, None)
}

/// Same as [`color_for_workspace`] but with an explicit manifest
/// snapshot. The async handler passes a fresh `load_all` result
/// per focus event; tests pass a static fixture so the manifest
/// lookup is deterministic.
#[must_use]
pub fn color_for_workspace_with_manifests(
    store: &TagStore,
    ws_num: i32,
    manifests: Option<&[crate::config::TagManifest]>,
) -> String {
    let owning_tag = find_owning_tag(store, ws_num);

    // Priority 1: tag-manifest border_color.
    if let (Some(tag), Some(ms)) = (owning_tag.as_ref(), manifests) {
        if let Some(m) = ms.iter().find(|m| m.name == tag.name) {
            if let Some(c) = m.border_color.as_deref() {
                if is_valid_hex_color(c) {
                    return c.to_string();
                }
            }
        }
    }

    // Priority 2: TagStore group_color (legacy path).
    owning_tag
        .and_then(|t| t.group_color.clone())
        .filter(|c| is_valid_hex_color(c))
        .unwrap_or_else(|| DEFAULT_FOCUSED_COLOR.to_string())
}

/// `true` if `s` looks like a CSS hex color: leading `#` + 3, 4, 6,
/// or 8 hex digits. The validation is intentionally strict so
/// malformed tag.json entries don't pass garbage to swayipc (which
/// would silently no-op and confuse operators).
#[must_use]
pub fn is_valid_hex_color(s: &str) -> bool {
    let rest = match s.strip_prefix('#') {
        Some(r) => r,
        None => return false,
    };
    if !matches!(rest.len(), 3 | 4 | 6 | 8) {
        return false;
    }
    rest.chars().all(|c| c.is_ascii_hexdigit())
}

/// Build the swayipc `client.focused` command string with the
/// given tint color. Per the R12-Q21 design lock:
///
///   `client.focused <color> <color> #f4f4f4 <color> <color>`
///
/// — border, background, text, indicator, child_border. Text
/// stays off-white for readability against any owning-tag color.
#[must_use]
pub fn client_focused_command(color: &str) -> String {
    format!(
        "client.focused {color} {color} {text} {color} {color}",
        color = color,
        text = FOCUSED_TEXT_COLOR
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::{Tag, TagFlavor, TagMember, TagStore};

    fn dev_tag_with_color(ws_num: i32, color: Option<&str>) -> Tag {
        Tag {
            name: "Dev".to_string(),
            flavor: TagFlavor::Manual,
            members: vec![TagMember::Workspace { num: ws_num }],
            group_color: color.map(String::from),
            preferred_output: None,
            default_layout: None,
            autostart: Vec::new(),
        }
    }

    /// Empty store / untagged workspace → default Carbon blue.
    /// Mirrors the bench acceptance "focus an untagged workspace
    /// → border returns to Carbon blue".
    #[test]
    fn untagged_workspace_returns_default_carbon_blue() {
        let store = TagStore::default();
        assert_eq!(color_for_workspace(&store, 1), DEFAULT_FOCUSED_COLOR);
    }

    /// Owning tag with valid group_color → returns the tag's color.
    /// Mirrors the bench acceptance "focus a Dev-tag workspace →
    /// focused window border turns Dev color".
    #[test]
    fn tagged_workspace_returns_tag_color() {
        let mut store = TagStore::default();
        store.add(dev_tag_with_color(1, Some("#42be65"))).unwrap();
        assert_eq!(color_for_workspace(&store, 1), "#42be65");
    }

    /// Owning tag with no `group_color` set → falls back to default.
    #[test]
    fn owning_tag_without_color_falls_back_to_default() {
        let mut store = TagStore::default();
        store.add(dev_tag_with_color(1, None)).unwrap();
        assert_eq!(color_for_workspace(&store, 1), DEFAULT_FOCUSED_COLOR);
    }

    /// Owning tag with a MALFORMED `group_color` (not a hex
    /// string) → falls back to default rather than passing
    /// garbage to swayipc. Locks the strict-validation contract.
    #[test]
    fn malformed_color_falls_back_to_default() {
        let mut store = TagStore::default();
        store
            .add(dev_tag_with_color(1, Some("rebeccapurple")))
            .unwrap();
        assert_eq!(color_for_workspace(&store, 1), DEFAULT_FOCUSED_COLOR);

        let mut store2 = TagStore::default();
        store2.add(dev_tag_with_color(1, Some("#xyz"))).unwrap();
        assert_eq!(color_for_workspace(&store2, 1), DEFAULT_FOCUSED_COLOR);
    }

    /// Hex-color validator accepts every standard form (#rgb,
    /// #rgba, #rrggbb, #rrggbbaa) and rejects everything else.
    #[test]
    fn hex_color_validator_locks_recognised_forms() {
        // Accepted forms — 3, 4, 6, 8 hex digits after `#`.
        assert!(is_valid_hex_color("#f00"));
        assert!(is_valid_hex_color("#f00f"));
        assert!(is_valid_hex_color("#42be65"));
        assert!(is_valid_hex_color("#42be65ff"));
        assert!(is_valid_hex_color("#FFFFFF"));
        // Rejected forms.
        assert!(!is_valid_hex_color("42be65")); // missing leading #
        assert!(!is_valid_hex_color("#42be6")); // 5 chars — not a recognised length
        assert!(!is_valid_hex_color("#42")); // 2 chars
        assert!(!is_valid_hex_color("#1234567")); // 7 chars
        assert!(!is_valid_hex_color("#x42")); // non-hex char
        assert!(!is_valid_hex_color("")); // empty string
        assert!(!is_valid_hex_color("#")); // bare # with no digits
        assert!(!is_valid_hex_color("rebeccapurple")); // CSS named color
    }

    /// `client.focused` command shape matches the R12-Q21 design
    /// lock: 5 color slots, text fixed at `#f4f4f4`, the other
    /// 4 take the tint.
    #[test]
    fn client_focused_command_matches_r12_q21_shape() {
        let cmd = client_focused_command("#42be65");
        assert_eq!(
            cmd,
            "client.focused #42be65 #42be65 #f4f4f4 #42be65 #42be65"
        );
        // Default-color round-trip.
        let cmd_default = client_focused_command(DEFAULT_FOCUSED_COLOR);
        assert_eq!(
            cmd_default,
            "client.focused #2b9af3 #2b9af3 #f4f4f4 #2b9af3 #2b9af3"
        );
    }

    /// Tag owns multiple workspaces → all resolve to the same
    /// color (locking the "tag color spans every owned workspace"
    /// contract).
    #[test]
    fn tag_color_spans_every_owned_workspace() {
        let mut store = TagStore::default();
        let t = Tag {
            name: "Dev".to_string(),
            flavor: TagFlavor::Manual,
            members: vec![
                TagMember::Workspace { num: 1 },
                TagMember::Workspace { num: 2 },
                TagMember::Workspace { num: 3 },
            ],
            group_color: Some("#33b1ff".to_string()),
            preferred_output: None,
            default_layout: None,
            autostart: Vec::new(),
        };
        store.add(t).unwrap();
        for ws in 1..=3 {
            assert_eq!(color_for_workspace(&store, ws), "#33b1ff");
        }
        // Untagged ws 4 → default.
        assert_eq!(color_for_workspace(&store, 4), DEFAULT_FOCUSED_COLOR);
    }

    // ── HYP-22.sway-bridge — tag-manifest border_color takes
    //    precedence over the legacy TagStore group_color ───────

    fn dev_manifest_with(border_color: Option<&str>) -> crate::config::TagManifest {
        crate::config::TagManifest {
            name: "Dev".to_string(),
            border_color: border_color.map(|s| s.to_string()),
            ..crate::config::TagManifest::default()
        }
    }

    /// When a manifest carries a valid `border_color`, it wins
    /// over the TagStore's `group_color`.
    #[test]
    fn manifest_border_color_overrides_tagstore_group_color() {
        let mut store = TagStore::default();
        store
            .add(dev_tag_with_color(1, Some("#tagstore_color")))
            .unwrap();
        // TagStore color is invalid (intentional, to confirm
        // the manifest path doesn't fall through to a malformed
        // legacy entry).
        // Manifest carries the canonical Carbon green.
        let manifests = vec![dev_manifest_with(Some("#42be65"))];
        let c = color_for_workspace_with_manifests(&store, 1, Some(&manifests));
        assert_eq!(c, "#42be65");
    }

    /// When the manifest has no `border_color`, fall through to
    /// the TagStore's `group_color`.
    #[test]
    fn manifest_without_border_color_falls_through_to_tagstore() {
        let mut store = TagStore::default();
        store.add(dev_tag_with_color(1, Some("#abcdef"))).unwrap();
        let manifests = vec![dev_manifest_with(None)];
        let c = color_for_workspace_with_manifests(&store, 1, Some(&manifests));
        assert_eq!(c, "#abcdef");
    }

    /// Manifest border_color must be a valid hex — malformed
    /// values fall through to TagStore as if the field was None.
    #[test]
    fn manifest_with_malformed_border_color_falls_through() {
        let mut store = TagStore::default();
        store.add(dev_tag_with_color(1, Some("#abcdef"))).unwrap();
        // Manifest carries garbage that fails the hex predicate.
        let manifests = vec![dev_manifest_with(Some("not-a-color"))];
        let c = color_for_workspace_with_manifests(&store, 1, Some(&manifests));
        assert_eq!(c, "#abcdef");
    }

    /// When neither the manifest nor TagStore has a usable color,
    /// fall through to the platform default.
    #[test]
    fn no_color_anywhere_returns_default() {
        let mut store = TagStore::default();
        store.add(dev_tag_with_color(1, None)).unwrap();
        let manifests = vec![dev_manifest_with(None)];
        let c = color_for_workspace_with_manifests(&store, 1, Some(&manifests));
        assert_eq!(c, DEFAULT_FOCUSED_COLOR);
    }

    /// When the manifest snapshot is None (early boot / load
    /// failure), the function behaves exactly like the original
    /// `color_for_workspace` — TagStore is the only source.
    #[test]
    fn none_manifests_means_tagstore_only() {
        let mut store = TagStore::default();
        store.add(dev_tag_with_color(1, Some("#42be65"))).unwrap();
        let c = color_for_workspace_with_manifests(&store, 1, None);
        assert_eq!(c, "#42be65");
    }

    /// Manifest lookup must match by tag NAME — a manifest for
    /// a different tag doesn't bleed into this workspace.
    #[test]
    fn manifest_for_different_tag_is_ignored() {
        let mut store = TagStore::default();
        store.add(dev_tag_with_color(1, Some("#abcdef"))).unwrap();
        // Manifest is for a different tag name.
        let manifests = vec![crate::config::TagManifest {
            name: "Other".to_string(),
            border_color: Some("#000000".to_string()),
            ..crate::config::TagManifest::default()
        }];
        let c = color_for_workspace_with_manifests(&store, 1, Some(&manifests));
        // Falls through to TagStore.
        assert_eq!(c, "#abcdef");
    }
}
