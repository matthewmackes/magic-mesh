//! Card schema (Portal-31, R5-Q3).
//!
//! 12 fields, one schema_version, one ID, untyped `metadata` bucket
//! for forward-compatible drift.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Current card schema version, stamped at write time. (Moved here from
/// the deleted `migration` module — GUI-4; the registry that consumed it
/// was never reached, but the field is live wire format.)
pub const SCHEMA_VERSION: u32 = 1;

/// One Object in the universal card subsystem.
///
/// Fields (count = 12):
/// 1. `id`                 — stable mesh-merged identifier.
/// 2. `schema_version`     — [`SCHEMA_VERSION`] at write time.
/// 3. `kind`               — discriminator (app / file / peer / …).
/// 4. `title`              — primary user-visible label.
/// 5. `subtitle`           — secondary label (optional).
/// 6. `body`               — long-form text (optional).
/// 7. `icon`               — Material Symbols glyph name or icon resource ref.
/// 8. `tags`               — Portal-18 tag handles.
/// 9. `metadata`           — untyped map for forward-compat / enrich.
/// 10. `children`          — composition (R5-Q10).
/// 11. `created_ts`        — Unix epoch seconds at first creation.
/// 12. `updated_ts`        — Unix epoch seconds at last edit.
///
/// Cards serialize via `serde_json` so the LizardFS mesh store
/// ships them around the mesh as plain `.json` files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    /// Stable mesh-merged identifier (see [`crate::stable_id_for`]).
    pub id: String,

    /// Schema version this card was written under. Default = current
    /// crate version.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,

    /// What kind of Object this Card represents.
    pub kind: CardKind,

    /// Primary user-visible label.
    pub title: String,

    /// Optional secondary label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,

    /// Optional long-form body text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,

    /// Optional icon ref — Material Symbols glyph name or path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,

    /// Portal-18 tag handles attached to this card.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// Free-form metadata bucket. Preserves any unknown fields written
    /// by a newer mesh peer — consumers drain known fields
    /// into typed places and leaves the rest here (R10-Q37).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,

    /// Composed children — same Card recursively (R5-Q10).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Card>,

    /// Unix epoch seconds at first creation.
    #[serde(default)]
    pub created_ts: u64,

    /// Unix epoch seconds at last edit.
    #[serde(default)]
    pub updated_ts: u64,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

/// Discriminator for the kind of Object this Card represents.
///
/// Open-set with [`CardKind::Other`] so a newer peer can introduce a
/// new kind without breaking older readers — the deserializer falls
/// back into `Other(String)` so a newer kind round-trips losslessly; a consumer either upgrades
/// or preserves the value (R10-Q37).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    /// Desktop application (`.desktop`).
    App,
    /// File on disk or mesh-home.
    File,
    /// Mesh peer.
    Peer,
    /// Activity record (notification, event, log entry).
    Activity,
    /// Contact (VOIP).
    Contact,
    /// Podman container.
    Container,
    /// Sway workspace.
    Workspace,
    /// SNI tray item.
    Tray,
    /// Pinned-zone segment.
    Zone,
    /// Free-form note.
    Note,
    /// Network host discovered by EPIC-MESH-PROBE (a mesh peer, LAN
    /// device, or operator-arbitrary target). Probe facts live in
    /// `metadata`; see [`crate::probe`].
    Host,
    /// A network service on a [`CardKind::Host`] (one open port +
    /// its nmap-identified product/version). Rendered as a child Card
    /// under its host. Probe facts live in `metadata`.
    Service,
    /// Forward-compat — an unknown kind from a newer peer. Round-trips
    /// through serde so we never lose it.
    #[serde(untagged)]
    Other(String),
}

impl CardKind {
    /// Canonical lowercase tag for ID hashing.  Stable across crate
    /// versions; new variants must extend, never re-order.
    pub fn tag(&self) -> &str {
        match self {
            Self::App => "app",
            Self::File => "file",
            Self::Peer => "peer",
            Self::Activity => "activity",
            Self::Contact => "contact",
            Self::Container => "container",
            Self::Workspace => "workspace",
            Self::Tray => "tray",
            Self::Zone => "zone",
            Self::Note => "note",
            Self::Host => "host",
            Self::Service => "service",
            Self::Other(s) => s.as_str(),
        }
    }
}

impl Card {
    /// Convenience constructor with sane defaults: schema_version is
    /// current, timestamps both = `now_ts`, no children, no tags.
    pub fn new(kind: CardKind, title: impl Into<String>, now_ts: u64) -> Self {
        let title = title.into();
        let id = crate::id::stable_id_for(&kind, &title);
        Self {
            id,
            schema_version: SCHEMA_VERSION,
            kind,
            title,
            subtitle: None,
            body: None,
            icon: None,
            tags: Vec::new(),
            metadata: BTreeMap::new(),
            children: Vec::new(),
            created_ts: now_ts,
            updated_ts: now_ts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_card_uses_current_schema_version() {
        let c = Card::new(CardKind::Note, "hello", 100);
        assert_eq!(c.schema_version, SCHEMA_VERSION);
        assert_eq!(c.created_ts, 100);
        assert_eq!(c.updated_ts, 100);
        assert!(c.children.is_empty());
        assert!(c.tags.is_empty());
    }

    #[test]
    fn new_card_id_is_stable_for_same_inputs() {
        let a = Card::new(CardKind::App, "Firefox", 1);
        let b = Card::new(CardKind::App, "Firefox", 9_999);
        assert_eq!(a.id, b.id, "ID depends only on kind + title");
    }

    #[test]
    fn new_card_id_differs_when_kind_changes() {
        let a = Card::new(CardKind::App, "thing", 0);
        let b = Card::new(CardKind::File, "thing", 0);
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn card_round_trips_through_json() {
        let mut c = Card::new(CardKind::Peer, "host2", 42);
        c.subtitle = Some("nebula".into());
        c.tags.push("starred".into());
        c.metadata
            .insert("hostname".into(), serde_json::json!("host2.mesh.mde"));

        let raw = serde_json::to_string(&c).unwrap();
        let back: Card = serde_json::from_str(&raw).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn json_omits_empty_optional_fields() {
        let c = Card::new(CardKind::Note, "x", 0);
        let raw = serde_json::to_value(&c).unwrap();
        let obj = raw.as_object().unwrap();
        assert!(!obj.contains_key("subtitle"));
        assert!(!obj.contains_key("body"));
        assert!(!obj.contains_key("icon"));
        assert!(!obj.contains_key("tags"));
        assert!(!obj.contains_key("metadata"));
        assert!(!obj.contains_key("children"));
    }

    #[test]
    fn unknown_metadata_field_round_trips() {
        // Newer peer wrote `flavor: "ginger"` — older reader must
        // preserve it through the metadata bucket.
        let raw = serde_json::json!({
            "id": "abc",
            "schema_version": 1,
            "kind": "note",
            "title": "t",
            "created_ts": 1,
            "updated_ts": 1,
            "metadata": { "flavor": "ginger" }
        });
        let c: Card = serde_json::from_value(raw).unwrap();
        assert_eq!(c.metadata.get("flavor"), Some(&serde_json::json!("ginger")));
        let back = serde_json::to_value(&c).unwrap();
        assert_eq!(back["metadata"]["flavor"], serde_json::json!("ginger"));
    }

    #[test]
    fn unknown_kind_variant_round_trips() {
        // Newer peer wrote kind="hologram" — Other(_) catches it.
        let raw = serde_json::json!({
            "id": "abc",
            "schema_version": 1,
            "kind": "hologram",
            "title": "t",
            "created_ts": 1,
            "updated_ts": 1
        });
        let c: Card = serde_json::from_value(raw).unwrap();
        assert_eq!(c.kind, CardKind::Other("hologram".into()));
    }

    #[test]
    fn children_compose_recursively() {
        let mut parent = Card::new(CardKind::Note, "parent", 0);
        parent.children.push(Card::new(CardKind::Note, "c1", 0));
        parent.children.push(Card::new(CardKind::Note, "c2", 0));
        assert_eq!(parent.children.len(), 2);

        let raw = serde_json::to_string(&parent).unwrap();
        let back: Card = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.children.len(), 2);
        assert_eq!(back.children[0].title, "c1");
    }

    #[test]
    fn card_kind_tag_is_stable() {
        assert_eq!(CardKind::App.tag(), "app");
        assert_eq!(CardKind::File.tag(), "file");
        assert_eq!(CardKind::Peer.tag(), "peer");
        assert_eq!(CardKind::Activity.tag(), "activity");
        assert_eq!(CardKind::Contact.tag(), "contact");
        assert_eq!(CardKind::Container.tag(), "container");
        assert_eq!(CardKind::Workspace.tag(), "workspace");
        assert_eq!(CardKind::Tray.tag(), "tray");
        assert_eq!(CardKind::Zone.tag(), "zone");
        assert_eq!(CardKind::Note.tag(), "note");
        assert_eq!(CardKind::Host.tag(), "host");
        assert_eq!(CardKind::Service.tag(), "service");
        assert_eq!(CardKind::Other("custom".into()).tag(), "custom");
    }

    #[test]
    fn card_field_count_is_twelve() {
        // R5-Q3 lock — exactly 12 named fields. This is a structural
        // assertion: bumping the count is a schema-version bump.
        let c = Card::new(CardKind::Note, "x", 0);
        let raw = serde_json::to_value(&c).unwrap();
        // The default serialization elides empty optionals; force them
        // all to non-empty so we count every field.
        let mut full = c.clone();
        full.subtitle = Some("s".into());
        full.body = Some("b".into());
        full.icon = Some("i".into());
        full.tags.push("t".into());
        full.metadata.insert("k".into(), serde_json::json!(1));
        full.children.push(Card::new(CardKind::Note, "c", 0));
        let raw = serde_json::to_value(&full).unwrap();
        let fields = raw.as_object().unwrap();
        assert_eq!(fields.len(), 12, "card schema lock: 12 fields exactly");
    }
}
