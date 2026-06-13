//! Fleet → Inventory panel — node roster pulled from `mackesd
//! nodes list --json` (CB-1.5.a). Mirrors the v1.x
//! `mackes/workbench/fleet/inventory.py` panel.
//!
//! Backend surface: the binary subcommand was added to
//! `crates/mackesd/src/bin/mackesd.rs` as `Cmd::Nodes { ...
//! NodesCmd::List { json } }`. Phase E will move every workbench
//! shell-out to a direct zbus surface; until then the subprocess
//! pattern matches the fleet_settings + fleet_revisions panels.

use cosmic::iced::widget::{column, container, row, scrollable, text};
use cosmic::iced::{Element, Length, Padding, Task};
use mde_theme::{EmptyState, Icon};
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};
use crate::panel_chrome::{empty_state, panel_container, status_badge, BadgeSeverity};

/// One row of the inventory list — projection of the JSON object
/// `mackesd nodes list --json` emits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeRow {
    pub node_id: String,
    pub name: String,
    pub role: String,
    pub health: String,
    pub region: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct InventoryPanel {
    pub rows: Vec<NodeRow>,
    pub status: String,
    pub busy: bool,
    /// EFF-45 — set when the node-roster LOAD failed (vs legitimately
    /// empty). The view renders the error state instead of the
    /// misleading "No peers enrolled" empty state.
    pub load_error: Option<String>,
    /// `node_id` of the row the user has drilled into via
    /// `peers-why`. `None` = list view; `Some(id)` = drill-in
    /// view rendering the per-edge reason chain.
    pub focused: Option<String>,
    pub focus_report: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<NodeRow>),
    Error(String),
    FocusRow(String),
    FocusLoaded(String),
    Back,
    RefreshClicked,
}

impl InventoryPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                // EFF-45 — distinguish a CLI failure (load error) from an
                // empty roster (legitimately no peers enrolled yet).
                match run_mackesd_result(&["nodes", "list", "--json"]).await {
                    Err(e) => Message::Error(e),
                    Ok(raw) => match parse_nodes_json(&raw) {
                        Ok(rows) => Message::Loaded(rows),
                        Err(e) => Message::Error(e),
                    },
                }
            },
            crate::Message::Inventory,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(rows) => {
                self.rows = rows;
                self.status.clear();
                self.load_error = None;
                self.busy = false;
                Task::none()
            }
            Message::Error(msg) => {
                // EFF-45 — a failed load is an ERROR state, never an empty roster.
                self.load_error = Some(msg);
                self.busy = false;
                Task::none()
            }
            Message::FocusRow(node_id) => {
                self.focused = Some(node_id.clone());
                self.focus_report.clear();
                self.busy = true;
                self.status = "Loading peer detail…".into();
                Task::perform(
                    async move {
                        let raw = run_mackesd(&["peers-why", &node_id]).await;
                        Message::FocusLoaded(raw)
                    },
                    crate::Message::Inventory,
                )
            }
            Message::FocusLoaded(report) => {
                self.focus_report = report;
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::Back => {
                self.focused = None;
                self.focus_report.clear();
                self.status.clear();
                Task::none()
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Refreshing…".into();
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message, cosmic::Theme> {
        if let Some(node_id) = &self.focused {
            return self.view_focus(node_id);
        }
        self.view_list()
    }

    fn view_list(&self) -> Element<'_, crate::Message, cosmic::Theme> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;
        // UX-7.a — refresh routed through the shared button variant.
        let refresh_btn = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.busy).then(|| crate::Message::Inventory(Message::RefreshClicked)),
            palette,
        );

        // EFF-45 — a failed load renders as failure, never as the
        // "No peers enrolled" empty state.
        if let Some(err) = &self.load_error {
            return panel_container(
                crate::panel_chrome::error_state(err.clone(), palette, || {
                    crate::Message::Inventory(Message::RefreshClicked)
                }),
                density,
            );
        }

        if self.rows.is_empty() {
            // UX-6 — canonical empty-state for fleet panels.
            let _ = refresh_btn;
            let state = EmptyState::with_cta(
                "No peers enrolled",
                "Enroll a peer with `mackesd enroll --passcode <16-char>` on the \
                 joining node, then refresh to see it appear here.",
                "Refresh",
            )
            .with_icon(Icon::Fleet);
            return panel_container(
                empty_state(state, palette, || {
                    crate::Message::Inventory(Message::RefreshClicked)
                }),
                density,
            );
        }

        let header = row![
            text("node_id").width(Length::Fixed(220.0)),
            text("name").width(Length::Fixed(200.0)),
            text("role").width(Length::Fixed(100.0)),
            text("health").width(Length::Fixed(100.0)),
            text("region"),
        ]
        .spacing(12);

        let rows = self.rows.iter().fold(column![], |col, row_data| {
            // UX-7.a — per-row Detail routed through Ghost.
            let drill = {
                let id = row_data.node_id.clone();
                variant_button(
                    "Detail",
                    ButtonVariant::Ghost,
                    Some(crate::Message::Inventory(Message::FocusRow(id))),
                    palette,
                )
            };
            // UX-6 — health renders as a pill-shaped status
            // badge sized to the value. Severity maps from the
            // existing health-glyph routing so the colour stays
            // honest with the underlying status.
            let severity = health_severity(&row_data.health);
            col.push(
                row![
                    text(&row_data.node_id).width(Length::Fixed(220.0)),
                    text(&row_data.name).width(Length::Fixed(200.0)),
                    text(&row_data.role).width(Length::Fixed(100.0)),
                    container(status_badge(
                        health_glyph(&row_data.health),
                        severity,
                        palette,
                    ))
                    .width(Length::Fixed(100.0)),
                    text(row_data.region.as_deref().unwrap_or("-")).width(Length::Fixed(120.0)),
                    drill,
                ]
                .spacing(12),
            )
        });

        column![
            header,
            scrollable(container(rows.spacing(6))).height(Length::Fill),
            row![
                refresh_btn,
                text(&self.status).size(13),
                text(format!("Peers: {}", self.rows.len())).size(13),
            ]
            .spacing(12),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }

    fn view_focus(&self, node_id: &str) -> Element<'_, crate::Message, cosmic::Theme> {
        // UX-7.a — back routed through Ghost (low-emphasis nav).
        let back_btn = variant_button(
            "← Back to roster",
            ButtonVariant::Ghost,
            Some(crate::Message::Inventory(Message::Back)),
            crate::live_theme::palette(),
        );
        column![
            row![back_btn, text(format!("Peer detail — {node_id}")).size(18),].spacing(12),
            text(&self.status).size(13),
            scrollable(
                container(text(&self.focus_report).size(13))
                    .padding(Padding::new(12.0))
                    .width(Length::Fill),
            )
            .height(Length::Fill),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

/// Health-glyph mapper — one-character UI tag for each health
/// state the store records. Kept narrow on purpose; the
/// per-state colour lives in [`health_style`].
fn health_glyph(health: &str) -> String {
    match health {
        "healthy" => "● healthy".into(),
        "degraded" => "◐ degraded".into(),
        "unreachable" => "○ unreachable".into(),
        "unknown" => "? unknown".into(),
        other => format!("? {other}"),
    }
}

/// UX-6 — route a health string to a status-badge severity so
/// the inventory pill renders with the same colour vocabulary
/// the rest of the workbench uses (success / warning / danger /
/// neutral) instead of the panel-local `health_color()` palette.
fn health_severity(health: &str) -> BadgeSeverity {
    match health {
        "healthy" => BadgeSeverity::Success,
        "degraded" => BadgeSeverity::Warning,
        "unreachable" => BadgeSeverity::Danger,
        _ => BadgeSeverity::Neutral,
    }
}

/// Pure JSON parser for `mackesd nodes list --json` payloads.
/// Returns a friendly error message instead of bubbling
/// serde_json::Error so the reducer can lay it into the panel
/// status row without crashing.
///
/// # Errors
///
/// Returns an `Err(String)` when the input isn't a JSON array
/// of objects with the expected fields. Empty arrays / empty
/// input both produce `Ok(vec![])`.
pub fn parse_nodes_json(raw: &str) -> Result<Vec<NodeRow>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let v: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| format!("invalid mackesd output: {e}"))?;
    let arr = v
        .as_array()
        .ok_or_else(|| "expected JSON array at top level".to_string())?;
    Ok(arr
        .iter()
        .map(|obj| NodeRow {
            node_id: obj
                .get("node_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            name: obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            role: obj
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            health: obj
                .get("health")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            region: obj.get("region").and_then(|v| {
                if v.is_null() {
                    None
                } else {
                    v.as_str().map(str::to_string)
                }
            }),
        })
        .filter(|r| !r.node_id.is_empty())
        .collect())
}

/// Shell out to `mackesd` with the given args; returns stdout on
/// success, an empty string on any failure mode. The reducer
/// surfaces the empty as an Error message via the panel
/// status row.
pub async fn run_mackesd(args: &[&str]) -> String {
    let Ok(output) = Command::new("mackesd").args(args).output().await else {
        return String::new();
    };
    if !output.status.success() {
        return String::from_utf8(output.stderr).unwrap_or_default();
    }
    String::from_utf8(output.stdout).unwrap_or_default()
}

/// EFF-45 — honest version of [`run_mackesd`]: returns `Err` when the
/// command cannot be spawned or exits non-zero, so callers can distinguish
/// a real CLI failure from an empty (but valid) response.
pub async fn run_mackesd_result(args: &[&str]) -> Result<String, String> {
    let output = Command::new("mackesd")
        .args(args)
        .output()
        .await
        .map_err(|e| format!("mackesd not found or could not be spawned: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8(output.stderr).unwrap_or_default();
        return Err(if stderr.trim().is_empty() {
            format!("mackesd exited with status {}", output.status)
        } else {
            stderr
        });
    }
    Ok(String::from_utf8(output.stdout).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
        {"node_id": "peer:alpha", "name": "alpha-host", "public_key": "ABC=",
         "role": "host", "health": "healthy", "region": "us-east"},
        {"node_id": "peer:beta",  "name": "beta-host",  "public_key": "DEF=",
         "role": "peer", "health": "degraded", "region": null}
    ]"#;

    #[test]
    fn parse_nodes_json_extracts_every_field() {
        let rows = parse_nodes_json(SAMPLE).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].node_id, "peer:alpha");
        assert_eq!(rows[0].name, "alpha-host");
        assert_eq!(rows[0].role, "host");
        assert_eq!(rows[0].health, "healthy");
        assert_eq!(rows[0].region.as_deref(), Some("us-east"));
        assert_eq!(rows[1].region, None);
    }

    #[test]
    fn parse_nodes_json_empty_array_returns_empty_vec() {
        assert!(parse_nodes_json("[]").unwrap().is_empty());
        assert!(parse_nodes_json("").unwrap().is_empty());
        assert!(parse_nodes_json("   \n   ").unwrap().is_empty());
    }

    #[test]
    fn parse_nodes_json_rejects_non_array() {
        let err = parse_nodes_json("{\"x\": 1}").unwrap_err();
        assert!(err.contains("array"));
    }

    #[test]
    fn parse_nodes_json_rejects_garbage() {
        let err = parse_nodes_json("not json").unwrap_err();
        assert!(err.contains("invalid"));
    }

    #[test]
    fn parse_nodes_json_filters_rows_missing_node_id() {
        let raw = r#"[
            {"node_id": "peer:alpha", "name": "a", "role": "peer", "health": "healthy"},
            {"name": "no-id", "role": "peer", "health": "healthy"}
        ]"#;
        let rows = parse_nodes_json(raw).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].node_id, "peer:alpha");
    }

    #[test]
    fn parse_nodes_json_defaults_unknown_role_and_health() {
        let raw = r#"[{"node_id": "peer:x", "name": "x"}]"#;
        let rows = parse_nodes_json(raw).unwrap();
        assert_eq!(rows[0].role, "unknown");
        assert_eq!(rows[0].health, "unknown");
    }

    #[test]
    fn health_glyph_covers_every_locked_state() {
        assert!(health_glyph("healthy").contains("healthy"));
        assert!(health_glyph("degraded").contains("degraded"));
        assert!(health_glyph("unreachable").contains("unreachable"));
        assert!(health_glyph("unknown").contains("unknown"));
        // Unknown vendor states get a friendly "? <other>" so
        // panel rendering doesn't blank out on a schema bump.
        assert!(health_glyph("borked").contains("borked"));
    }

    #[test]
    fn loaded_message_clears_busy_and_records_rows() {
        let mut panel = InventoryPanel::new();
        panel.busy = true;
        let rows = parse_nodes_json(SAMPLE).unwrap();
        let _ = panel.update(Message::Loaded(rows.clone()));
        assert_eq!(panel.rows, rows);
        assert!(!panel.busy);
        assert!(panel.status.is_empty());
    }

    #[test]
    fn error_message_clears_busy_and_stores_load_error() {
        // EFF-45 — a load error goes to load_error, not status.
        let mut panel = InventoryPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Error("mackesd not on PATH".into()));
        assert_eq!(panel.load_error.as_deref(), Some("mackesd not on PATH"));
        assert!(panel.status.is_empty());
        assert!(!panel.busy);
    }

    #[test]
    fn loaded_message_clears_load_error() {
        // EFF-45 — a successful reload must clear any prior load_error.
        let mut panel = InventoryPanel::new();
        panel.load_error = Some("prior failure".into());
        let rows = parse_nodes_json(SAMPLE).unwrap();
        let _ = panel.update(Message::Loaded(rows));
        assert!(panel.load_error.is_none());
    }

    #[test]
    fn focus_row_sets_focused_and_busy() {
        let mut panel = InventoryPanel::new();
        let _ = panel.update(Message::FocusRow("peer:alpha".into()));
        assert_eq!(panel.focused.as_deref(), Some("peer:alpha"));
        assert!(panel.busy);
    }

    #[test]
    fn focus_loaded_clears_busy_and_stores_report() {
        let mut panel = InventoryPanel::new();
        panel.busy = true;
        panel.focused = Some("peer:alpha".into());
        let _ = panel.update(Message::FocusLoaded("{\"why\": \"…\"}".into()));
        assert!(!panel.busy);
        assert!(panel.focus_report.contains("why"));
    }

    #[test]
    fn back_clears_focus_state() {
        let mut panel = InventoryPanel::new();
        panel.focused = Some("peer:alpha".into());
        panel.focus_report = "stale".into();
        let _ = panel.update(Message::Back);
        assert!(panel.focused.is_none());
        assert!(panel.focus_report.is_empty());
    }

    #[test]
    fn refresh_while_busy_is_noop() {
        let mut panel = InventoryPanel::new();
        panel.busy = true;
        panel.status = "Refreshing…".into();
        let _ = panel.update(Message::RefreshClicked);
        assert_eq!(panel.status, "Refreshing…");
    }
}
