//! PLANES-23 — the **Node roles** panel (Provisioning plane).
//!
//! A fleet view of each node's pinned deployment role + its capability
//! tags (W58 — the read side of the W26 tag surface): the roster from
//! `mackesd nodes list --json` joined with the replicated cap-tags store
//! (`<root>/node-tags/<host>.json`, via `mackes_mesh_types::cap_tags`).
//! Links conceptually to install profiles (a role pin + tags is what a
//! profile bakes).
//!
//! Build-now-defer-visual: the load + join are pure and unit-tested; the
//! interactive **tag editor** (write side, `mackesd tag --set`) + the
//! on-Cosmic `/preview` are the deferred tail.

use std::path::PathBuf;

use iced::widget::{column, row, scrollable, text};
use iced::{Element, Length, Task};
use serde::Deserialize;

use crate::controls::{variant_button, ButtonVariant};
use crate::panel_chrome::panel_container;
use crate::panels::fleet_settings::run_mackesd;

/// One roster row from `mackesd nodes list --json` (subset).
#[derive(Debug, Clone, Deserialize)]
struct NodeJson {
    #[serde(default)]
    node_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    role: String,
}

/// One row the panel renders: a node, its role, and its tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRoleRow {
    pub node_id: String,
    pub name: String,
    pub role: String,
    pub tags: Vec<String>,
}

/// `MDE_WORKGROUP_ROOT`-or-`/mnt/mesh-storage` (matches the sibling panels).
#[must_use]
pub fn workgroup_root() -> PathBuf {
    std::env::var_os("MDE_WORKGROUP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/mesh-storage"))
}

/// Parse `mackesd nodes list --json` + join each node with its tags from
/// the cap-tags store under `root`. Pure over (json, root) for testing.
#[must_use]
pub fn build_rows(nodes_json: &str, root: &std::path::Path) -> Vec<NodeRoleRow> {
    let nodes: Vec<NodeJson> = serde_json::from_str(nodes_json).unwrap_or_default();
    let mut rows: Vec<NodeRoleRow> = nodes
        .into_iter()
        .map(|n| {
            // tags are keyed by hostname (the cap-tags store filename stem).
            let tags = mackes_mesh_types::cap_tags::read_tags(root, &n.name);
            let tag_names: Vec<String> = tags.tags.iter().map(|t| t.as_str().to_string()).collect();
            NodeRoleRow {
                node_id: n.node_id,
                name: n.name,
                role: n.role,
                tags: tag_names,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

/// The Node-roles panel state.
#[derive(Debug, Clone, Default)]
pub struct NodeRolesPanel {
    pub rows: Vec<NodeRoleRow>,
    pub loaded: bool,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<NodeRoleRow>),
    Error(String),
    RefreshClicked,
}

impl NodeRolesPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                match run_mackesd(&["nodes".into(), "list".into(), "--json".into()]).await {
                    Ok(out) => Message::Loaded(build_rows(&out, &workgroup_root())),
                    Err(e) => Message::Error(e),
                }
            },
            crate::Message::NodeRoles,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(rows) => {
                self.rows = rows;
                self.loaded = true;
                self.busy = false;
                self.status.clear();
                Task::none()
            }
            Message::Error(e) => {
                self.status = e;
                self.busy = false;
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

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;
        let refresh = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.busy).then(|| crate::Message::NodeRoles(Message::RefreshClicked)),
            palette,
        );

        let header = row![
            text("node").width(Length::Fixed(220.0)).size(13),
            text("role").width(Length::Fixed(120.0)).size(13),
            text("tags").size(13),
        ]
        .spacing(12);

        let mut list = column![header].spacing(6);
        if self.rows.is_empty() {
            list = list.push(
                text(if self.status.is_empty() {
                    "No nodes enrolled yet.".to_string()
                } else {
                    self.status.clone()
                })
                .size(13),
            );
        }
        for r in &self.rows {
            let tags = if r.tags.is_empty() {
                "(none)".to_string()
            } else {
                r.tags.join(", ")
            };
            list = list.push(
                row![
                    text(r.name.clone()).width(Length::Fixed(220.0)).size(13),
                    text(r.role.clone()).width(Length::Fixed(120.0)).size(13),
                    text(tags).size(13),
                ]
                .spacing(12),
            );
        }

        panel_container(
            column![
                row![text("Node roles & tags").size(20), refresh]
                    .spacing(12)
                    .align_y(iced::Alignment::Center),
                scrollable(list).height(Length::Fill),
            ]
            .spacing(16)
            .width(Length::Fill)
            .into(),
            density,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::cap_tags::{write_tags, CapabilityTag, NodeTags};

    #[test]
    fn build_rows_joins_roster_with_tags_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        // pine carries the execution tag; oak has none.
        let mut tags = NodeTags::default();
        tags.tags.insert(CapabilityTag::Execution);
        write_tags(tmp.path(), "pine", &tags).unwrap();

        let json = r#"[
            {"node_id":"peer:pine","name":"pine","role":"workstation"},
            {"node_id":"peer:oak","name":"oak","role":"server"}
        ]"#;
        let rows = build_rows(json, tmp.path());
        assert_eq!(rows.len(), 2);
        // sorted by name: oak, pine
        assert_eq!(rows[0].name, "oak");
        assert!(rows[0].tags.is_empty());
        assert_eq!(rows[1].name, "pine");
        assert_eq!(rows[1].role, "workstation");
        assert_eq!(rows[1].tags, vec!["execution"]);
    }

    #[test]
    fn build_rows_tolerates_garbage_json() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(build_rows("not json", tmp.path()).is_empty());
        assert!(build_rows("", tmp.path()).is_empty());
    }

    #[test]
    fn loaded_sets_rows_and_clears_busy() {
        let mut p = NodeRolesPanel::new();
        p.busy = true;
        let _ = p.update(Message::Loaded(vec![NodeRoleRow {
            node_id: "peer:x".into(),
            name: "x".into(),
            role: "host".into(),
            tags: vec![],
        }]));
        assert!(p.loaded);
        assert!(!p.busy);
        assert_eq!(p.rows.len(), 1);
    }
}
