//! PLANES-23 — the **Node roles** panel (Provisioning plane).
//!
//! A fleet view of each node's pinned deployment role + its capability
//! tags (W58 — the read side of the W26 tag surface): the roster from
//! `mackesd nodes list --json` joined with the replicated cap-tags store
//! (`<root>/node-tags/<host>.json`, via `mackes_mesh_types::cap_tags`).
//! Links conceptually to install profiles (a role pin + tags is what a
//! profile bakes).
//!
//! The interactive **tag editor** (W26 write side) lands here: each
//! node's v1 tags (hop / execution / headless / hypervisor, W82 +
//! DATACENTER-17) render as toggle buttons that shell `mackesd tag
//! --host <h> --set <new-set>` (any enrolled surface may set any
//! target's tags, W83) and reload. The `hypervisor` tag is how an
//! XCP-ng dom0 shows as a Hypervisor in the roster.

use std::collections::BTreeMap;

use cosmic::iced::widget::{column, row, scrollable, text};
use cosmic::iced::{Length, Task};
use cosmic::Element;
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

/// The v1 capability-tag vocabulary the editor toggles (W82; `hypervisor`
/// added by DATACENTER-17 so an XCP-ng dom0 shows as a Hypervisor). Mirrors
/// `mackes_mesh_types::cap_tags::CapabilityTag` — the set `mackesd tag --set`
/// gates on; a toggle the writer can't parse would be a dead button.
pub const V1_TAGS: [&str; 4] = ["hop", "execution", "headless", "hypervisor"];

/// The tag set after toggling `tag` on a node currently carrying
/// `current` — the comma-joined value handed to `mackesd tag --set`
/// (which REPLACES the set). Pure for testing.
#[must_use]
pub fn next_tag_set(current: &[String], tag: &str) -> String {
    let mut set: std::collections::BTreeSet<&str> = current.iter().map(String::as_str).collect();
    if !set.remove(tag) {
        set.insert(tag);
    }
    set.into_iter().collect::<Vec<_>>().join(",")
}

/// Invert the `mackesd tags --json` census (`[{tag, nodes:[…]}]`) into a
/// `host → [tags]` map, so the read uses the SAME server-resolved root
/// the `mackesd tag --set` write goes to (was: a direct cap-tags read at
/// a hardcoded `/mnt/mesh-storage`, which drifts from the write root on a
/// box where they differ).
#[must_use]
fn census_to_host_tags(tags_census_json: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Ok(census) = serde_json::from_str::<Vec<serde_json::Value>>(tags_census_json) else {
        return out;
    };
    for entry in census {
        let Some(tag) = entry.get("tag").and_then(|v| v.as_str()) else {
            continue;
        };
        for node in entry
            .get("nodes")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            if let Some(h) = node.as_str() {
                out.entry(h.to_string()).or_default().push(tag.to_string());
            }
        }
    }
    out
}

/// Parse `mackesd nodes list --json` + join each node with its tags from
/// the `mackesd tags --json` census. Pure over (nodes, census) for tests.
#[must_use]
pub fn build_rows(nodes_json: &str, tags_census_json: &str) -> Vec<NodeRoleRow> {
    let nodes: Vec<NodeJson> = serde_json::from_str(nodes_json).unwrap_or_default();
    let host_tags = census_to_host_tags(tags_census_json);
    let mut rows: Vec<NodeRoleRow> = nodes
        .into_iter()
        .map(|n| {
            let mut tags = host_tags.get(&n.name).cloned().unwrap_or_default();
            tags.sort();
            NodeRoleRow {
                node_id: n.node_id,
                name: n.name,
                role: n.role,
                tags,
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
    /// Toggle `tag` on node `name` (W26 write side).
    ToggleTag {
        name: String,
        tag: String,
    },
    /// Result of a `mackesd tag --set` write.
    TagApplied(Result<String, String>),
}

impl NodeRolesPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let nodes =
                    match run_mackesd(&["nodes".into(), "list".into(), "--json".into()]).await {
                        Ok(out) => out,
                        Err(e) => return Message::Error(e),
                    };
                // The tag census reads the same root `mackesd tag --set`
                // writes to; a census failure just means "no tags".
                let census = run_mackesd(&["tags".into(), "--json".into()])
                    .await
                    .unwrap_or_default();
                Message::Loaded(build_rows(&nodes, &census))
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
            Message::ToggleTag { name, tag } => {
                if self.busy {
                    return Task::none();
                }
                // Compute the new replacement set from this node's row.
                let Some(row) = self.rows.iter().find(|r| r.name == name) else {
                    return Task::none();
                };
                let new_set = next_tag_set(&row.tags, &tag);
                self.busy = true;
                self.status = format!("Setting {name} tags…");
                Task::perform(
                    async move {
                        run_mackesd(&["tag".into(), "--host".into(), name, "--set".into(), new_set])
                            .await
                    },
                    |r| crate::Message::NodeRoles(Message::TagApplied(r)),
                )
            }
            Message::TagApplied(Ok(_)) => {
                // Reload so the row reflects the persisted tag set.
                self.busy = true;
                self.status = "Refreshing…".into();
                Self::load()
            }
            Message::TagApplied(Err(e)) => {
                self.busy = false;
                self.status = format!("Tag write failed: {e}");
                Task::none()
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
            // Each v1 tag is a toggle: Primary (filled) when the node
            // carries it, Secondary (outline) when not. Disabled while a
            // write/refresh is in flight.
            let mut tag_toggles = row![].spacing(6);
            for tag in V1_TAGS {
                let has = r.tags.iter().any(|t| t == tag);
                let variant = if has {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Secondary
                };
                let msg = (!self.busy).then(|| {
                    crate::Message::NodeRoles(Message::ToggleTag {
                        name: r.name.clone(),
                        tag: tag.to_string(),
                    })
                });
                tag_toggles = tag_toggles.push(variant_button(tag, variant, msg, palette));
            }
            list = list.push(
                row![
                    text(r.name.clone()).width(Length::Fixed(220.0)).size(13),
                    text(r.role.clone()).width(Length::Fixed(120.0)).size(13),
                    tag_toggles,
                ]
                .spacing(12)
                .align_y(cosmic::iced::Alignment::Center),
            );
        }

        panel_container(
            column![
                row![text("Node roles & tags").size(20), refresh]
                    .spacing(12)
                    .align_y(cosmic::iced::Alignment::Center),
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

    #[test]
    fn build_rows_joins_roster_with_census_sorted() {
        // pine carries execution + hop; oak has none.
        let census = r#"[
            {"tag":"hop","nodes":["pine"]},
            {"tag":"execution","nodes":["pine"]},
            {"tag":"headless","nodes":[]}
        ]"#;
        let json = r#"[
            {"node_id":"peer:pine","name":"pine","role":"workstation"},
            {"node_id":"peer:oak","name":"oak","role":"server"}
        ]"#;
        let rows = build_rows(json, census);
        assert_eq!(rows.len(), 2);
        // sorted by name: oak, pine
        assert_eq!(rows[0].name, "oak");
        assert!(rows[0].tags.is_empty());
        assert_eq!(rows[1].name, "pine");
        assert_eq!(rows[1].role, "workstation");
        // tags sorted alphabetically.
        assert_eq!(rows[1].tags, vec!["execution", "hop"]);
    }

    #[test]
    fn v1_tags_match_the_typed_vocabulary() {
        // The editor toggles must be exactly the tags `mackesd tag --set`
        // can persist; a toggle outside CapabilityTag would be a dead button
        // (the write would bail "unknown capability tag"). DATACENTER-17.
        use mackes_mesh_types::cap_tags::CapabilityTag;
        let typed: Vec<&str> = CapabilityTag::ALL.iter().map(|t| t.as_str()).collect();
        assert_eq!(V1_TAGS.to_vec(), typed);
    }

    #[test]
    fn next_tag_set_adds_and_removes() {
        // Add execution to a node with none.
        assert_eq!(next_tag_set(&[], "execution"), "execution");
        // Toggle off a carried tag.
        assert_eq!(next_tag_set(&["execution".into()], "execution"), "");
        // Add a second tag — sorted, comma-joined (BTreeSet order).
        assert_eq!(next_tag_set(&["hop".into()], "execution"), "execution,hop");
        // Remove one of two, keeping the other.
        assert_eq!(
            next_tag_set(&["execution".into(), "headless".into()], "execution"),
            "headless"
        );
    }

    #[test]
    fn tag_write_failure_surfaces_in_status() {
        let mut p = NodeRolesPanel::new();
        p.busy = true;
        let _ = p.update(Message::TagApplied(Err("nope".into())));
        assert!(!p.busy);
        assert!(p.status.contains("nope"));
    }

    #[test]
    fn build_rows_tolerates_garbage_json() {
        assert!(build_rows("not json", "[]").is_empty());
        assert!(build_rows("", "also not json").is_empty());
        // Valid roster, garbage census → nodes with no tags.
        let rows = build_rows(r#"[{"node_id":"p","name":"p","role":"host"}]"#, "garbage");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].tags.is_empty());
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
