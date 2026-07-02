//! The last-writer-wins CRDT merge (locks Q2, Q4, Q5).
//!
//! A [`Collection`] is the converged state: a map of item id -> a bundle of
//! **LWW registers**, one per field. Applying an [`Op`] writes the op's value
//! into the touched registers *iff* its [`Hlc`] beats what is there; merging two
//! collections takes the higher-HLC register on each side. Both operations are
//! commutative, associative, and idempotent over the register set, so any node
//! that has seen the same op set converges to the same [`Collection`] regardless
//! of arrival order (the convergence property).
//!
//! Deletes are ordinary LWW registers, not tombstones (lock Q4): a
//! [`OpKind::DeleteItem`] writes `deleted = true`; a later edit (higher HLC)
//! writes a field and, because an add also writes `deleted = false`, a
//! higher-HLC re-add resurrects — the operator-accepted resurrection edge.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::hlc::{Author, Hlc};
use crate::model::{Bookmark, ContentHash, Folder, Item, ItemKind, Source};
use crate::op::{Op, OpKind};

/// A single last-writer-wins register: the winning value plus the [`Hlc`] and
/// [`Author`] that wrote it. An unset register (`None`) loses to any write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reg<T> {
    entry: Option<RegEntry<T>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RegEntry<T> {
    hlc: Hlc,
    author: Author,
    value: T,
}

impl<T> Default for Reg<T> {
    fn default() -> Self {
        Self { entry: None }
    }
}

impl<T: Clone> Reg<T> {
    /// Write `value` stamped `hlc`/`author`, keeping whichever stamp is greater.
    /// A strictly-greater HLC wins; an equal HLC (the identical op replayed) is
    /// idempotently kept, so replays never change the state.
    fn write(&mut self, hlc: &Hlc, author: &Author, value: T) {
        let beats = self.entry.as_ref().is_none_or(|cur| *hlc > cur.hlc);
        if beats {
            self.entry = Some(RegEntry {
                hlc: hlc.clone(),
                author: author.clone(),
                value,
            });
        }
    }

    /// Fold another register in (state-based merge): keep the higher HLC.
    fn merge(&mut self, other: &Self) {
        if let Some(o) = &other.entry {
            self.write(&o.hlc, &o.author, o.value.clone());
        }
    }

    /// The winning value, if the register was ever written.
    #[must_use]
    fn get(&self) -> Option<&T> {
        self.entry.as_ref().map(|e| &e.value)
    }

    /// The winning value, or `default` if unset.
    fn get_or(&self, default: T) -> T {
        self.entry.as_ref().map_or(default, |e| e.value.clone())
    }

    /// The stamp + author of the winning write, if any.
    fn meta(&self) -> Option<(&Hlc, &Author)> {
        self.entry.as_ref().map(|e| (&e.hlc, &e.author))
    }
}

/// The LWW register bundle for one item. `kind` is set once by the item's
/// Add op; the rest are independent LWW registers. Bookmark-only registers stay
/// unset on a folder and vice-versa.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemState {
    kind: Reg<ItemKind>,
    parent: Reg<Option<Uuid>>,
    order_key: Reg<String>,
    deleted: Reg<bool>,
    // Bookmark-only registers.
    url: Reg<String>,
    title: Reg<String>,
    favicon_ref: Reg<Option<ContentHash>>,
    tags: Reg<Vec<String>>,
    notes: Reg<String>,
    added: Reg<u64>,
    source: Reg<Source>,
    // Folder-only register.
    name: Reg<String>,
}

impl ItemState {
    /// The `(hlc, author)` of the most-recent winning write across all
    /// registers — the item's `modified` time and last author (lock Q64).
    fn latest(&self) -> Option<(&Hlc, &Author)> {
        [
            self.kind.meta(),
            self.parent.meta(),
            self.order_key.meta(),
            self.deleted.meta(),
            self.url.meta(),
            self.title.meta(),
            self.favicon_ref.meta(),
            self.tags.meta(),
            self.notes.meta(),
            self.added.meta(),
            self.source.meta(),
            self.name.meta(),
        ]
        .into_iter()
        .flatten()
        .max_by(|(a, _), (b, _)| a.cmp(b))
    }

    /// Merge another item's registers in (state-based CRDT join).
    fn merge(&mut self, other: &Self) {
        self.kind.merge(&other.kind);
        self.parent.merge(&other.parent);
        self.order_key.merge(&other.order_key);
        self.deleted.merge(&other.deleted);
        self.url.merge(&other.url);
        self.title.merge(&other.title);
        self.favicon_ref.merge(&other.favicon_ref);
        self.tags.merge(&other.tags);
        self.notes.merge(&other.notes);
        self.added.merge(&other.added);
        self.source.merge(&other.source);
        self.name.merge(&other.name);
    }

    /// Whether the item is currently live: it has a known kind (its Add has
    /// arrived) and is not LWW-deleted.
    fn is_live(&self) -> bool {
        self.kind.get().is_some() && !self.deleted.get_or(false)
    }
}

/// The converged bookmark collection: item id -> its LWW register bundle
/// (locks Q2, Q4, Q5).
///
/// Build one, [`Collection::apply`] ops into it, and read the tree back with
/// [`Collection::items`] / [`Collection::children`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    items: BTreeMap<Uuid, ItemState>,
}

impl Collection {
    /// An empty collection.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one op, writing its value into the touched LWW registers iff its
    /// HLC beats what is there. Idempotent: replaying an op is a no-op.
    pub fn apply(&mut self, op: &Op) {
        let st = self.items.entry(op.target()).or_default();
        let h = &op.hlc;
        let a = &op.author;
        match &op.kind {
            OpKind::AddBookmark {
                parent,
                order_key,
                url,
                title,
                favicon_ref,
                tags,
                notes,
                added,
                source,
                ..
            } => {
                st.kind.write(h, a, ItemKind::Bookmark);
                st.parent.write(h, a, *parent);
                st.order_key.write(h, a, order_key.clone());
                st.url.write(h, a, url.clone());
                st.title.write(h, a, title.clone());
                st.favicon_ref.write(h, a, *favicon_ref);
                st.tags.write(h, a, tags.clone());
                st.notes.write(h, a, notes.clone());
                st.added.write(h, a, *added);
                st.source.write(h, a, source.clone());
                // An add also LWW-competes on `deleted`, so a higher-HLC re-add
                // resurrects a deleted id (lock Q4).
                st.deleted.write(h, a, false);
            }
            OpKind::EditBookmark {
                url,
                title,
                favicon_ref,
                tags,
                notes,
                ..
            } => {
                if let Some(v) = url {
                    st.url.write(h, a, v.clone());
                }
                if let Some(v) = title {
                    st.title.write(h, a, v.clone());
                }
                if let Some(v) = favicon_ref {
                    st.favicon_ref.write(h, a, *v);
                }
                if let Some(v) = tags {
                    st.tags.write(h, a, v.clone());
                }
                if let Some(v) = notes {
                    st.notes.write(h, a, v.clone());
                }
            }
            OpKind::MoveItem {
                parent, order_key, ..
            } => {
                st.parent.write(h, a, *parent);
                st.order_key.write(h, a, order_key.clone());
            }
            OpKind::DeleteItem { .. } => {
                st.deleted.write(h, a, true);
            }
            OpKind::AddFolder {
                name,
                parent,
                order_key,
                ..
            } => {
                st.kind.write(h, a, ItemKind::Folder);
                st.name.write(h, a, name.clone());
                st.parent.write(h, a, *parent);
                st.order_key.write(h, a, order_key.clone());
                st.deleted.write(h, a, false);
            }
            OpKind::RenameFolder { name, .. } => {
                st.name.write(h, a, name.clone());
            }
        }
    }

    /// Apply many ops, in whatever order they arrive.
    pub fn apply_all<'a, I>(&mut self, ops: I)
    where
        I: IntoIterator<Item = &'a Op>,
    {
        for op in ops {
            self.apply(op);
        }
    }

    /// Merge another converged collection in (state-based CRDT join): the
    /// higher-HLC register wins on every field. Equivalent to applying the union
    /// of both sides' op sets.
    pub fn merge(&mut self, other: &Self) {
        for (id, other_state) in &other.items {
            self.items.entry(*id).or_default().merge(other_state);
        }
    }

    /// Build the converged [`Item`] for an id, if it is live (its Add arrived
    /// and it is not LWW-deleted).
    #[must_use]
    pub fn item(&self, id: Uuid) -> Option<Item> {
        let st = self.items.get(&id)?;
        if !st.is_live() {
            return None;
        }
        let (modified, last_author) = st.latest().map_or_else(
            || (0, Author::new(String::new(), String::new())),
            |(h, a)| (h.wall_ms, a.clone()),
        );
        match st.kind.get()? {
            ItemKind::Bookmark => Some(Item::Bookmark(Bookmark {
                id,
                parent: st.parent.get_or(None),
                order_key: st.order_key.get_or(String::new()),
                url: st.url.get_or(String::new()),
                title: st.title.get_or(String::new()),
                favicon_ref: st.favicon_ref.get_or(None),
                tags: st.tags.get_or(Vec::new()),
                notes: st.notes.get_or(String::new()),
                added: st.added.get_or(0),
                modified,
                source: st.source.get_or(Source::Manual),
                last_author,
            })),
            ItemKind::Folder => Some(Item::Folder(Folder {
                id,
                name: st.name.get_or(String::new()),
                parent: st.parent.get_or(None),
                order_key: st.order_key.get_or(String::new()),
                last_author,
            })),
        }
    }

    /// Every live item, sorted by id (a stable, position-independent order for
    /// snapshots and equality checks).
    #[must_use]
    pub fn items(&self) -> Vec<Item> {
        self.items.keys().filter_map(|id| self.item(*id)).collect()
    }

    /// The live children of `parent` (`None` = top level), sorted by
    /// fractional-index `order_key` then id (lock Q3) — the render order.
    #[must_use]
    pub fn children(&self, parent: Option<Uuid>) -> Vec<Item> {
        let mut kids: Vec<Item> = self
            .items()
            .into_iter()
            .filter(|it| it.parent() == parent)
            .collect();
        kids.sort_by(|x, y| {
            x.order_key()
                .cmp(y.order_key())
                .then_with(|| x.id().cmp(&y.id()))
        });
        kids
    }

    /// The live top-level items (children of the implicit root), render-ordered.
    #[must_use]
    pub fn roots(&self) -> Vec<Item> {
        self.children(None)
    }

    /// The number of live items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.values().filter(|s| s.is_live()).count()
    }

    /// Whether there are no live items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hlc::HlcClock;

    fn author(node: &str) -> Author {
        Author::new("user".into(), node.into())
    }

    fn add_bookmark(id: Uuid, parent: Option<Uuid>, order_key: &str, title: &str) -> OpKind {
        OpKind::AddBookmark {
            id,
            parent,
            order_key: order_key.into(),
            url: format!("https://{title}.example"),
            title: title.into(),
            favicon_ref: None,
            tags: vec![],
            notes: String::new(),
            added: 0,
            source: Source::Manual,
        }
    }

    #[test]
    fn add_then_edit_lww_picks_the_higher_hlc() {
        let id = Uuid::from_u128(1);
        let mut c = Collection::new();
        c.apply(&Op::new(
            Hlc::new(1, 0, "n1".into()),
            author("n1"),
            add_bookmark(id, None, "a", "old"),
        ));
        c.apply(&Op::new(
            Hlc::new(5, 0, "n1".into()),
            author("n1"),
            OpKind::EditBookmark {
                id,
                url: None,
                title: Some("new".into()),
                favicon_ref: None,
                tags: None,
                notes: None,
            },
        ));
        // A stale lower-HLC edit must lose.
        c.apply(&Op::new(
            Hlc::new(3, 0, "n1".into()),
            author("n1"),
            OpKind::EditBookmark {
                id,
                url: None,
                title: Some("stale".into()),
                favicon_ref: None,
                tags: None,
                notes: None,
            },
        ));
        let Some(Item::Bookmark(b)) = c.item(id) else {
            unreachable!("expected a live bookmark");
        };
        assert_eq!(b.title, "new", "higher HLC wins, stale write ignored");
        assert_eq!(b.modified, 5);
    }

    #[test]
    fn delete_is_lww_and_a_later_edit_resurrects() {
        let id = Uuid::from_u128(2);
        let mut c = Collection::new();
        c.apply(&Op::new(
            Hlc::new(1, 0, "n1".into()),
            author("n1"),
            add_bookmark(id, None, "a", "t"),
        ));
        c.apply(&Op::new(
            Hlc::new(2, 0, "n1".into()),
            author("n1"),
            OpKind::DeleteItem { id },
        ));
        assert!(c.item(id).is_none(), "deleted -> not live");
        // A higher-HLC edit competes with the delete; but EditBookmark alone
        // does not clear `deleted`, so it stays gone...
        c.apply(&Op::new(
            Hlc::new(3, 0, "n1".into()),
            author("n1"),
            OpKind::EditBookmark {
                id,
                url: None,
                title: Some("edited".into()),
                favicon_ref: None,
                tags: None,
                notes: None,
            },
        ));
        assert!(c.item(id).is_none(), "edit does not resurrect a delete");
        // ...but a higher-HLC re-add writes deleted=false and resurrects.
        c.apply(&Op::new(
            Hlc::new(4, 0, "n1".into()),
            author("n1"),
            add_bookmark(id, None, "a", "reborn"),
        ));
        let Some(Item::Bookmark(b)) = c.item(id) else {
            unreachable!("expected resurrection");
        };
        assert_eq!(b.title, "reborn");
    }

    #[test]
    fn author_attribution_is_preserved_through_lww() {
        let id = Uuid::from_u128(3);
        let mut c = Collection::new();
        c.apply(&Op::new(
            Hlc::new(1, 0, "n1".into()),
            Author::new("alice".into(), "n1".into()),
            add_bookmark(id, None, "a", "t"),
        ));
        c.apply(&Op::new(
            Hlc::new(9, 0, "n2".into()),
            Author::new("bob".into(), "n2".into()),
            OpKind::EditBookmark {
                id,
                url: None,
                title: Some("bobs-edit".into()),
                favicon_ref: None,
                tags: None,
                notes: None,
            },
        ));
        let Some(Item::Bookmark(b)) = c.item(id) else {
            unreachable!("expected a live bookmark");
        };
        assert_eq!(b.last_author.user, "bob", "last winning writer attributed");
        assert_eq!(b.last_author.node, "n2");
    }

    #[test]
    fn children_render_in_fractional_index_order() {
        let mut c = Collection::new();
        let folder = Uuid::from_u128(100);
        c.apply(&Op::new(
            Hlc::new(1, 0, "n1".into()),
            author("n1"),
            OpKind::AddFolder {
                id: folder,
                name: "F".into(),
                parent: None,
                order_key: "a".into(),
            },
        ));
        // Add three bookmarks with out-of-order insertion but ordered keys.
        for (n, key, title) in [
            (10u128, "c", "third"),
            (11, "a", "first"),
            (12, "b", "second"),
        ] {
            c.apply(&Op::new(
                Hlc::new(2, 0, "n1".into()),
                author("n1"),
                add_bookmark(Uuid::from_u128(n), Some(folder), key, title),
            ));
        }
        let kids = c.children(Some(folder));
        let titles: Vec<String> = kids
            .iter()
            .filter_map(|it| match it {
                Item::Bookmark(b) => Some(b.title.clone()),
                Item::Folder(_) => None,
            })
            .collect();
        assert_eq!(titles, vec!["first", "second", "third"]);
    }

    #[test]
    fn concurrent_edits_from_two_nodes_converge_regardless_of_order() {
        // Two nodes edit the same bookmark title concurrently; the higher HLC
        // must win no matter which order each replica applies the ops.
        let id = Uuid::from_u128(7);
        let base = Op::new(
            Hlc::new(1, 0, "n1".into()),
            author("n1"),
            add_bookmark(id, None, "a", "base"),
        );
        let e1 = Op::new(
            Hlc::new(5, 0, "n1".into()),
            author("n1"),
            OpKind::EditBookmark {
                id,
                url: None,
                title: Some("from-n1".into()),
                favicon_ref: None,
                tags: None,
                notes: None,
            },
        );
        let e2 = Op::new(
            Hlc::new(5, 0, "n2".into()),
            author("n2"),
            OpKind::EditBookmark {
                id,
                url: None,
                title: Some("from-n2".into()),
                favicon_ref: None,
                tags: None,
                notes: None,
            },
        );
        let mut a = Collection::new();
        a.apply_all([&base, &e1, &e2]);
        let mut b = Collection::new();
        b.apply_all([&e2, &e1, &base]);
        assert_eq!(a, b, "same ops, different order -> identical state");
        // n2 > n1 at equal (wall, counter) via the node-id tiebreak.
        let Some(Item::Bookmark(bm)) = a.item(id) else {
            unreachable!("expected a live bookmark");
        };
        assert_eq!(bm.title, "from-n2");
    }

    #[test]
    fn merge_matches_apply_all() {
        // A state-based merge of two replicas equals applying the union.
        let id = Uuid::from_u128(8);
        let base = Op::new(
            Hlc::new(1, 0, "n1".into()),
            author("n1"),
            add_bookmark(id, None, "a", "base"),
        );
        let mv = Op::new(
            Hlc::new(4, 0, "n2".into()),
            author("n2"),
            OpKind::MoveItem {
                id,
                parent: None,
                order_key: "z".into(),
            },
        );
        let mut left = Collection::new();
        left.apply(&base);
        let mut right = Collection::new();
        right.apply_all([&base, &mv]);
        left.merge(&right);
        let mut union = Collection::new();
        union.apply_all([&base, &mv]);
        assert_eq!(left, union);
    }

    #[test]
    fn hlc_clock_stamped_ops_stay_monotonic_and_converge() {
        // Sanity: ops minted by a real HlcClock feed the merge cleanly.
        let mut clk = HlcClock::new("n1".into());
        let id = Uuid::from_u128(9);
        let o1 = Op::new(
            clk.tick(100),
            author("n1"),
            add_bookmark(id, None, "a", "one"),
        );
        let o2 = Op::new(
            clk.tick(100),
            author("n1"),
            OpKind::RenameFolder {
                id,
                name: "ignored".into(),
            },
        );
        assert!(o1.hlc < o2.hlc);
    }
}
