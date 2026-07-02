//! The append-only op set (locks Q64, and the op list in the design doc).
//!
//! Every mutation to the collection is an [`Op`] — an immutable record carrying
//! its [`Hlc`] stamp (lock Q5) and [`Author`] (lock Q64) plus a typed
//! [`OpKind`]. Ops are the only thing that crosses the mesh (as Syncthing op
//! segments, BOOKMARKS-2); the CRDT ([`crate::crdt`]) folds a *set* of them into
//! a converged tree, order-independently. Nothing here mutates in place — an
//! edit is a new op that LWW-competes with the old one.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::hlc::{Author, Hlc};
use crate::model::{ContentHash, Source};

/// One append-only mutation: its clock stamp, its author, and what it does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Op {
    /// The Hybrid Logical Clock stamp deciding LWW order (lock Q5).
    pub hlc: Hlc,
    /// The authenticated user + node that wrote this op (lock Q64).
    pub author: Author,
    /// What the op does.
    pub kind: OpKind,
}

impl Op {
    /// Assemble an op from its stamp, author, and body.
    #[must_use]
    pub const fn new(hlc: Hlc, author: Author, kind: OpKind) -> Self {
        Self { hlc, author, kind }
    }

    /// The id of the item this op targets.
    #[must_use]
    pub const fn target(&self) -> Uuid {
        self.kind.target()
    }
}

/// A single-field edit: `Some(v)` sets the field, `None` leaves it untouched
/// (so an [`OpKind::EditBookmark`] carries only the fields that changed, each
/// LWW-competing independently).
pub type Edit<T> = Option<T>;

/// The typed body of an [`Op`].
///
/// The set is closed: `AddBookmark`, `EditBookmark`, `MoveItem`, `DeleteItem`,
/// `AddFolder`, `RenameFolder`. Deletes are ops too (lock Q4: no tombstones — a
/// delete LWW-competes on the item's `deleted` register, and a later edit
/// resurrects).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OpKind {
    /// Create a bookmark leaf (lock Q1 mints the id; lock Q3 the order key).
    AddBookmark {
        /// The new item id.
        id: Uuid,
        /// Parent folder, or `None` for top-level.
        parent: Option<Uuid>,
        /// Fractional-index order key among siblings.
        order_key: String,
        /// Target URL.
        url: String,
        /// Display title.
        title: String,
        /// Content-addressed favicon reference, if known.
        favicon_ref: Option<ContentHash>,
        /// Tags kept from import.
        tags: Vec<String>,
        /// Free-text notes.
        notes: String,
        /// Wall time (ms) added.
        added: u64,
        /// Origin of the bookmark.
        source: Source,
    },
    /// Edit one or more bookmark fields (each LWW-competes independently). A
    /// `None` field is left untouched.
    EditBookmark {
        /// The bookmark to edit.
        id: Uuid,
        /// New URL, if changed.
        url: Edit<String>,
        /// New title, if changed.
        title: Edit<String>,
        /// New favicon reference, if changed (`Some(None)` clears it).
        favicon_ref: Edit<Option<ContentHash>>,
        /// New tag set, if changed.
        tags: Edit<Vec<String>>,
        /// New notes, if changed.
        notes: Edit<String>,
    },
    /// Reparent and/or reorder an item — one op for a drag (lock Q3).
    MoveItem {
        /// The item to move.
        id: Uuid,
        /// New parent folder, or `None` for top-level.
        parent: Option<Uuid>,
        /// New fractional-index order key.
        order_key: String,
    },
    /// Delete an item (lock Q4: no tombstone — competes on the `deleted`
    /// register by HLC).
    DeleteItem {
        /// The item to delete.
        id: Uuid,
    },
    /// Create a folder (lock Q1 mints the id; lock Q3 the order key).
    AddFolder {
        /// The new folder id.
        id: Uuid,
        /// Display name.
        name: String,
        /// Parent folder, or `None` for top-level.
        parent: Option<Uuid>,
        /// Fractional-index order key among siblings.
        order_key: String,
    },
    /// Rename a folder.
    RenameFolder {
        /// The folder to rename.
        id: Uuid,
        /// The new name.
        name: String,
    },
}

impl OpKind {
    /// The id of the item this op targets.
    #[must_use]
    pub const fn target(&self) -> Uuid {
        match self {
            Self::AddBookmark { id, .. }
            | Self::EditBookmark { id, .. }
            | Self::MoveItem { id, .. }
            | Self::DeleteItem { id }
            | Self::AddFolder { id, .. }
            | Self::RenameFolder { id, .. } => *id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_round_trips_through_serde() {
        let op = Op::new(
            Hlc::new(1, 0, "n1".into()),
            Author::new("alice".into(), "n1".into()),
            OpKind::AddFolder {
                id: Uuid::nil(),
                name: "Imported".into(),
                parent: None,
                order_key: "a".into(),
            },
        );
        let json = serde_json::to_string(&op).expect("serialize");
        let back: Op = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(op, back);
    }

    #[test]
    fn target_reads_the_id_for_every_kind() {
        let id = Uuid::from_u128(42);
        assert_eq!(OpKind::DeleteItem { id }.target(), id);
        assert_eq!(
            OpKind::RenameFolder {
                id,
                name: "x".into()
            }
            .target(),
            id
        );
    }
}
