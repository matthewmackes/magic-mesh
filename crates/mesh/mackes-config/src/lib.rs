//! Serde schema for `~/.config/mackes-panel/panel.toml`.
//!
//! Per the 50-question lock (Q18–Q22), this file lives in TOML, is mesh-
//! replicated whole-file via QNM-Shared, and is hot-reloaded by inotify
//! diff-and-apply. This crate carries the schema only — I/O and inotify
//! watching live in `mackes-panel`.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// Root document of `panel.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PanelConfig {
    /// Top status bar configuration (20 px, monochrome Material Symbols, Q13/Q14).
    #[serde(default)]
    pub top_bar: TopBarConfig,

    /// Bottom dock configuration (48 px icons, no magnification, Q11/Q12).
    #[serde(default)]
    pub dock: DockConfig,

    /// Mesh-sync behavior for this very file (Q18–Q21).
    #[serde(default)]
    pub mesh: MeshConfig,

    /// PC-10 — peer-card privacy + behaviour toggles.
    #[serde(default)]
    pub peer_card: PeerCardConfig,
}

/// PC-10 — peer connection card preferences. Controls which
/// enrichment sources the card may consult on peer-join.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerCardConfig {
    /// Online enrichment master switch. When `false`, PC-5/6/7
    /// sources are skipped and the card renders hwdb-only.
    /// Default `true` per PC-10 lock.
    #[serde(default = "true_default")]
    pub online_enrichment: bool,
}

impl Default for PeerCardConfig {
    fn default() -> Self {
        Self {
            online_enrichment: true,
        }
    }
}

/// What lives in the top bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopBarConfig {
    /// Ordered list of status-cluster items rendered on the right
    /// (e.g. `["mesh", "clipboard", "volume", "battery", "notifications", "user"]`).
    #[serde(default = "default_status_items")]
    pub status_items: Vec<String>,

    /// Whether to render the global appmenu (`DBusMenu`) on the left.
    #[serde(default = "true_default")]
    pub appmenu: bool,
}

impl Default for TopBarConfig {
    fn default() -> Self {
        Self {
            status_items: default_status_items(),
            appmenu: true,
        }
    }
}

/// What lives in the dock, in render order. Apps and mesh resources are
/// interleaved per Q10 — no segmentation, no separator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DockConfig {
    /// Ordered list of dock entries. Each entry is either a `.desktop`
    /// reference or a `MeshResource` ID.
    #[serde(default)]
    pub items: Vec<DockItem>,
}

/// One slot in the dock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DockItem {
    /// A pinned application by `.desktop` ID
    /// (e.g. `firefox.desktop`, `org.gnome.Terminal.desktop`).
    App {
        /// The basename of the `.desktop` file under `applications/`.
        desktop: String,
    },

    /// A mesh resource by its `MeshResource::id()`
    /// (e.g. `peer:anvil`, `share:anvil:code`).
    Mesh {
        /// Stable ID emitted by `mackes_mesh_types::MeshResource::id()`.
        id: String,
    },
}

/// Mesh-sync behavior for the panel.toml file itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshConfig {
    /// Whether to mirror this file to `~/.qnm-sync/mackes-panel/panel.toml`
    /// (Q18 — defaults to true).
    #[serde(default = "true_default")]
    pub replicate: bool,

    /// How often to hash-compare against the peer mirrors. 0 disables drift
    /// detection. Default 300 (5 min) per Q22.
    #[serde(default = "default_drift_seconds")]
    pub drift_check_seconds: u32,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            replicate: true,
            drift_check_seconds: default_drift_seconds(),
        }
    }
}

const fn true_default() -> bool {
    true
}

const fn default_drift_seconds() -> u32 {
    300
}

fn default_status_items() -> Vec<String> {
    [
        "mesh",
        "clipboard",
        "volume",
        "battery",
        "notifications",
        "user",
    ]
    .iter()
    .map(|&s| s.to_owned())
    .collect()
}

/// Parse a `panel.toml` document. Unknown fields are silently dropped so
/// older binaries don't blow up on newer files.
///
/// # Errors
/// Returns the underlying `toml::de::Error` if the document is malformed.
pub fn parse(text: &str) -> Result<PanelConfig, toml::de::Error> {
    toml::from_str(text)
}

/// Pin a new `.desktop` to the end of the dock. No-op when the entry
/// is already pinned (idempotent by id).
pub fn pin_app(cfg: &mut PanelConfig, desktop: &str) {
    if cfg
        .dock
        .items
        .iter()
        .any(|i| matches!(i, DockItem::App { desktop: d } if d == desktop))
    {
        return;
    }
    cfg.dock.items.push(DockItem::App {
        desktop: desktop.to_owned(),
    });
}

/// Unpin a `.desktop` from the dock. No-op when no matching App entry
/// exists (idempotent by id). Mirrors `pin_app`. 1.0.7 — completes the
/// pin/unpin pair surfaced by the Workbench right-click menus.
pub fn unpin_app(cfg: &mut PanelConfig, desktop: &str) {
    cfg.dock
        .items
        .retain(|i| !matches!(i, DockItem::App { desktop: d } if d == desktop));
}

/// Move the dock item at `from` to position `to` (clamped to the
/// valid range). No-op if the indices are equal or out of range.
pub fn reorder_dock(cfg: &mut PanelConfig, from: usize, to: usize) {
    if from >= cfg.dock.items.len() || from == to {
        return;
    }
    let item = cfg.dock.items.remove(from);
    let target = to.min(cfg.dock.items.len());
    cfg.dock.items.insert(target, item);
}

/// Serialize a `PanelConfig` back to TOML — used by Phase 2.2 to write
/// the default `panel.toml` on first launch.
///
/// # Errors
/// Returns `toml::ser::Error` only on truly exotic schema breakage
/// (`PanelConfig` is a closed struct so this should not fail in
/// practice).
pub fn to_toml_string(cfg: &PanelConfig) -> Result<String, toml::ser::Error> {
    toml::to_string_pretty(cfg)
}

/// Build the first-launch default `PanelConfig`. Identical to `Default`
/// but spelled out so future presets can swap in their own variants.
#[must_use]
pub fn default_config() -> PanelConfig {
    PanelConfig {
        top_bar: TopBarConfig::default(),
        dock: DockConfig::default(),
        mesh: MeshConfig::default(),
        peer_card: PeerCardConfig::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_document_yields_defaults() {
        let cfg = parse("").expect("empty doc should parse");
        assert!(cfg.top_bar.appmenu);
        assert_eq!(cfg.top_bar.status_items.len(), 6);
        assert!(cfg.mesh.replicate);
        assert_eq!(cfg.mesh.drift_check_seconds, 300);
        assert!(cfg.dock.items.is_empty());
        // PC-10 — online enrichment defaults to true.
        assert!(cfg.peer_card.online_enrichment);
    }

    #[test]
    fn peer_card_online_enrichment_can_be_disabled() {
        // PC-10 — operator can disable online enrichment in the
        // config; mded's peer-join worker should then short-
        // circuit PC-5/6/7 and the card renders hwdb-only.
        let cfg =
            parse("[peer_card]\nonline_enrichment = false\n").expect("peer_card section parses");
        assert!(!cfg.peer_card.online_enrichment);
    }

    #[test]
    fn dock_items_round_trip() {
        let doc = r"
            [[dock.items]]
            kind    = 'app'
            desktop = 'firefox.desktop'

            [[dock.items]]
            kind = 'mesh'
            id   = 'peer:anvil'
        ";
        let cfg = parse(doc).expect("parse");
        assert_eq!(cfg.dock.items.len(), 2);
        assert_eq!(
            cfg.dock.items[0],
            DockItem::App {
                desktop: "firefox.desktop".into()
            }
        );
        assert_eq!(
            cfg.dock.items[1],
            DockItem::Mesh {
                id: "peer:anvil".into()
            }
        );
    }

    #[test]
    fn unknown_top_level_keys_are_tolerated() {
        let doc = r"
            [unknown_section]
            x = 1
        ";
        // Should NOT error. Unknown sections at the top level are ignored
        // by serde unless we explicitly forbid them (we don't).
        let _ = parse(doc).expect("tolerate unknown sections");
    }

    #[test]
    fn mesh_drift_can_be_disabled() {
        let doc = r"
            [mesh]
            drift_check_seconds = 0
        ";
        let cfg = parse(doc).expect("parse");
        assert_eq!(cfg.mesh.drift_check_seconds, 0);
        assert!(cfg.mesh.replicate); // default still applied
    }

    #[test]
    fn default_config_round_trips_through_toml() {
        let cfg = default_config();
        let text = to_toml_string(&cfg).expect("serialize default");
        let back = parse(&text).expect("re-parse default");
        assert_eq!(cfg, back);
    }

    #[test]
    fn pin_app_appends_when_missing() {
        let mut cfg = default_config();
        pin_app(&mut cfg, "firefox.desktop");
        assert_eq!(cfg.dock.items.len(), 1);
        assert!(matches!(
            cfg.dock.items[0],
            DockItem::App { ref desktop } if desktop == "firefox.desktop"
        ));
    }

    #[test]
    fn pin_app_is_idempotent_by_id() {
        let mut cfg = default_config();
        pin_app(&mut cfg, "firefox.desktop");
        pin_app(&mut cfg, "firefox.desktop");
        assert_eq!(cfg.dock.items.len(), 1);
    }

    #[test]
    fn reorder_dock_moves_within_bounds() {
        let mut cfg = default_config();
        pin_app(&mut cfg, "a.desktop");
        pin_app(&mut cfg, "b.desktop");
        pin_app(&mut cfg, "c.desktop");
        reorder_dock(&mut cfg, 2, 0);
        let names: Vec<&str> = cfg
            .dock
            .items
            .iter()
            .map(|i| match i {
                DockItem::App { desktop } => desktop.as_str(),
                DockItem::Mesh { .. } => "",
            })
            .collect();
        assert_eq!(names, vec!["c.desktop", "a.desktop", "b.desktop"]);
    }

    #[test]
    fn reorder_dock_clamps_out_of_range() {
        let mut cfg = default_config();
        pin_app(&mut cfg, "a.desktop");
        pin_app(&mut cfg, "b.desktop");
        // from > len — no-op
        reorder_dock(&mut cfg, 99, 0);
        assert_eq!(cfg.dock.items.len(), 2);
        // to > len — clamp to end
        reorder_dock(&mut cfg, 0, 99);
        assert!(matches!(
            cfg.dock.items[1],
            DockItem::App { ref desktop } if desktop == "a.desktop"
        ));
    }

    #[test]
    fn default_config_carries_six_status_items() {
        let cfg = default_config();
        assert_eq!(cfg.top_bar.status_items.len(), 6);
        assert!(cfg.top_bar.status_items.iter().any(|s| s == "mesh"));
        assert!(cfg
            .top_bar
            .status_items
            .iter()
            .any(|s| s == "notifications"));
    }

    #[test]
    fn unpin_app_removes_matching_entry() {
        let mut cfg = default_config();
        pin_app(&mut cfg, "firefox.desktop");
        pin_app(&mut cfg, "thunar.desktop");
        unpin_app(&mut cfg, "firefox.desktop");
        assert_eq!(cfg.dock.items.len(), 1);
        assert_eq!(
            cfg.dock.items[0],
            DockItem::App {
                desktop: "thunar.desktop".into()
            }
        );
    }

    #[test]
    fn unpin_app_is_idempotent_when_missing() {
        let mut cfg = default_config();
        pin_app(&mut cfg, "firefox.desktop");
        unpin_app(&mut cfg, "not-there.desktop");
        assert_eq!(cfg.dock.items.len(), 1);
    }

    #[test]
    fn unpin_app_leaves_mesh_entries_untouched() {
        let mut cfg = default_config();
        cfg.dock.items.push(DockItem::Mesh {
            id: "peer:anvil".into(),
        });
        pin_app(&mut cfg, "firefox.desktop");
        unpin_app(&mut cfg, "firefox.desktop");
        // Mesh entry must remain — unpin_app only sweeps App variants.
        assert_eq!(cfg.dock.items.len(), 1);
        assert_eq!(
            cfg.dock.items[0],
            DockItem::Mesh {
                id: "peer:anvil".into()
            }
        );
    }

    #[test]
    fn reorder_dock_noop_when_indices_equal() {
        let mut cfg = default_config();
        pin_app(&mut cfg, "a.desktop");
        pin_app(&mut cfg, "b.desktop");
        let before = cfg.dock.items.clone();
        reorder_dock(&mut cfg, 1, 1);
        assert_eq!(cfg.dock.items, before);
    }

    #[test]
    fn topbar_can_disable_appmenu() {
        let doc = r"
            [top_bar]
            appmenu = false
        ";
        let cfg = parse(doc).expect("parse");
        assert!(!cfg.top_bar.appmenu);
        // Status items default still in place.
        assert_eq!(cfg.top_bar.status_items.len(), 6);
    }

    #[test]
    fn topbar_custom_status_items_override_default() {
        let doc = r#"
            [top_bar]
            status_items = ["volume", "battery"]
        "#;
        let cfg = parse(doc).expect("parse");
        assert_eq!(cfg.top_bar.status_items, vec!["volume", "battery"]);
    }

    #[test]
    fn mesh_replicate_can_be_disabled() {
        let doc = r"
            [mesh]
            replicate = false
        ";
        let cfg = parse(doc).expect("parse");
        assert!(!cfg.mesh.replicate);
        // drift_check_seconds default preserved.
        assert_eq!(cfg.mesh.drift_check_seconds, 300);
    }

    #[test]
    fn malformed_toml_returns_error() {
        // Unbalanced bracket — parser must fail loudly.
        let doc = "[top_bar\n";
        assert!(parse(doc).is_err());
    }

    #[test]
    fn unknown_dock_item_kind_rejected() {
        // Tagged enum with rename_all=snake_case — `other` isn't a
        // declared variant, so deserialization must error.
        let doc = r"
            [[dock.items]]
            kind = 'other'
            payload = 'nope'
        ";
        assert!(parse(doc).is_err());
    }
}
