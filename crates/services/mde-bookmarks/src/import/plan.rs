//! The import → CRDT glue (§6: glue over the model, never a re-derivation).
//!
//! [`plan_import`] folds a browser-independent [`ParsedTree`] into a `Vec<Op>`
//! against the *current* [`Collection`], honouring the import locks:
//!
//!   * everything lands under `Imported/<Browser>` (lock Q12/Q16);
//!   * folders are reused by `(parent, name)` and bookmarks deduped by
//!     normalized URL (lock Q15), so a re-import adds only genuinely-new items —
//!     **idempotent** (lock Q16): running the same import twice yields no ops
//!     the second time;
//!   * a URL that already exists is kept in place; its title is refreshed only
//!     if the import carries a different, non-empty one (lock Q16);
//!   * ordering uses the model's fractional index ([`key_between`]).
//!
//! This module mints no ids or clocks of its own beyond the model's own
//! [`Uuid`] / [`HlcClock`] — it produces ops the CRDT already understands.

use std::collections::HashMap;

use uuid::Uuid;

use crate::{key_between, Author, Collection, HlcClock, Item, Op, OpKind, Source};

use super::normalize::normalize_url;
use super::parsed::{ParsedBookmark, ParsedNode, ParsedTree};

/// The top-level folder every import lands under (lock Q12/Q16).
const IMPORTED_ROOT: &str = "Imported";

/// What an import planned: the ops to apply plus a per-category count.
///
/// Applying [`ImportOutcome::ops`] to the collection they were planned against
/// produces the imported tree; the counts are for the honest import summary the
/// UI shows (BOOKMARKS-4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportOutcome {
    /// The ops to apply (in order) to realize the import.
    pub ops: Vec<Op>,
    /// New bookmarks created.
    pub bookmarks_added: usize,
    /// Bookmarks skipped because their normalized URL already existed.
    pub bookmarks_deduped: usize,
    /// Existing bookmarks whose title was refreshed from the import.
    pub bookmarks_refreshed: usize,
    /// Folders newly created (existing ones are reused, not duplicated).
    pub folders_created: usize,
}

/// The `<Browser>` subfolder name for a source (lock Q12).
fn browser_folder_name(source: &Source) -> String {
    match source {
        Source::Firefox => "Firefox".to_string(),
        Source::Chromium => "Chromium".to_string(),
        Source::Safari => "Safari".to_string(),
        Source::NetscapeHtml => "Netscape".to_string(),
        Source::Manual => IMPORTED_ROOT.to_string(),
        Source::Other(name) => name.clone(),
    }
}

/// Plan an import of `tree` into `collection` (the collection is not mutated).
///
/// Ops are stamped by `clock` (each at wall time `now_ms`; the HLC counter keeps
/// them strictly ordered) and attributed to `author`. Apply the returned
/// [`ImportOutcome::ops`] to realize the import.
#[must_use]
pub fn plan_import(
    collection: &Collection,
    tree: &ParsedTree,
    clock: &mut HlcClock,
    author: &Author,
    now_ms: u64,
) -> ImportOutcome {
    let browser = browser_folder_name(&tree.source);
    let mut planner = Planner::new(
        collection,
        tree.source.clone(),
        clock,
        author.clone(),
        now_ms,
    );
    let imported = planner.ensure_folder(None, IMPORTED_ROOT);
    let browser_id = planner.ensure_folder(Some(imported), &browser);
    for node in &tree.roots {
        planner.walk(browser_id, node);
    }
    planner.finish()
}

/// Folds a parsed tree into ops while tracking what already exists so the plan
/// stays idempotent + deduped.
struct Planner<'a> {
    collection: &'a Collection,
    source: Source,
    clock: &'a mut HlcClock,
    author: Author,
    now_ms: u64,
    ops: Vec<Op>,
    /// `(parent, name) -> folder id` for folder reuse (existing + freshly made).
    folder_index: HashMap<(Option<Uuid>, String), Uuid>,
    /// Last order key minted/seen under a parent, seeded lazily from the tree.
    last_key: HashMap<Option<Uuid>, Option<String>>,
    /// Normalized URL → bookmark id (existing + freshly added), for dedup.
    url_index: HashMap<String, Uuid>,
    /// Current title per bookmark id, so a refresh only fires on a real change.
    titles: HashMap<Uuid, String>,
    added: usize,
    deduped: usize,
    refreshed: usize,
    folders_created: usize,
}

impl<'a> Planner<'a> {
    fn new(
        collection: &'a Collection,
        source: Source,
        clock: &'a mut HlcClock,
        author: Author,
        now_ms: u64,
    ) -> Self {
        // Seed the reuse indexes from the existing converged tree so re-import is
        // idempotent (folders reused, URLs deduped).
        let mut folder_index = HashMap::new();
        let mut url_index = HashMap::new();
        let mut titles = HashMap::new();
        for item in collection.items() {
            match item {
                Item::Folder(f) => {
                    folder_index.entry((f.parent, f.name)).or_insert(f.id);
                }
                Item::Bookmark(b) => {
                    if let Some(key) = normalize_url(&b.url) {
                        url_index.entry(key).or_insert(b.id);
                    }
                    titles.insert(b.id, b.title);
                }
            }
        }
        Self {
            collection,
            source,
            clock,
            author,
            now_ms,
            ops: Vec::new(),
            folder_index,
            last_key: HashMap::new(),
            url_index,
            titles,
            added: 0,
            deduped: 0,
            refreshed: 0,
            folders_created: 0,
        }
    }

    /// Stamp `kind` with the next HLC + the author and record it.
    fn push(&mut self, kind: OpKind) {
        let hlc = self.clock.tick(self.now_ms);
        self.ops.push(Op::new(hlc, self.author.clone(), kind));
    }

    /// Mint an order key that appends after the last child of `parent` (seeding
    /// the tail from the existing collection the first time).
    fn next_key(&mut self, parent: Option<Uuid>) -> String {
        let last = if let Some(seen) = self.last_key.get(&parent) {
            seen.clone()
        } else {
            let seeded = self
                .collection
                .children(parent)
                .last()
                .map(|it| it.order_key().to_string());
            self.last_key.insert(parent, seeded.clone());
            seeded
        };
        let key = key_between(last.as_deref(), None);
        self.last_key.insert(parent, Some(key.clone()));
        key
    }

    /// Return the id of the `(parent, name)` folder, reusing an existing one or
    /// minting an `AddFolder` op for a new one (idempotent, lock Q16).
    fn ensure_folder(&mut self, parent: Option<Uuid>, name: &str) -> Uuid {
        let key = (parent, name.to_string());
        if let Some(&id) = self.folder_index.get(&key) {
            return id;
        }
        let id = Uuid::new_v4();
        let order_key = self.next_key(parent);
        self.push(OpKind::AddFolder {
            id,
            name: name.to_string(),
            parent,
            order_key,
        });
        self.folder_index.insert(key, id);
        self.folders_created += 1;
        id
    }

    /// Add a bookmark under `parent`, or dedup it against an existing URL
    /// (refreshing the title on a real change), per locks Q15/Q16.
    fn add_bookmark(&mut self, parent: Uuid, bookmark: &ParsedBookmark) {
        let Some(key) = normalize_url(&bookmark.url) else {
            // Unparseable URL (e.g. a Firefox internal `place:` query already
            // filtered upstream, or malformed): nothing to dedup on — skip.
            return;
        };
        if let Some(&existing) = self.url_index.get(&key) {
            self.deduped += 1;
            let changed = self
                .titles
                .get(&existing)
                .is_none_or(|cur| cur != &bookmark.title);
            if !bookmark.title.is_empty() && changed {
                self.push(OpKind::EditBookmark {
                    id: existing,
                    url: None,
                    title: Some(bookmark.title.clone()),
                    favicon_ref: None,
                    tags: None,
                    notes: None,
                });
                self.titles.insert(existing, bookmark.title.clone());
                self.refreshed += 1;
            }
            return;
        }
        let id = Uuid::new_v4();
        let order_key = self.next_key(Some(parent));
        self.push(OpKind::AddBookmark {
            id,
            parent: Some(parent),
            order_key,
            url: bookmark.url.clone(),
            title: bookmark.title.clone(),
            favicon_ref: None,
            tags: bookmark.tags.clone(),
            notes: String::new(),
            added: bookmark.added_ms,
            source: self.source.clone(),
        });
        self.url_index.insert(key, id);
        self.titles.insert(id, bookmark.title.clone());
        self.added += 1;
    }

    /// Recursively plan a node under `parent`.
    fn walk(&mut self, parent: Uuid, node: &ParsedNode) {
        match node {
            ParsedNode::Folder { name, children } => {
                let folder = self.ensure_folder(Some(parent), name);
                for child in children {
                    self.walk(folder, child);
                }
            }
            ParsedNode::Bookmark(bookmark) => self.add_bookmark(parent, bookmark),
        }
    }

    fn finish(self) -> ImportOutcome {
        ImportOutcome {
            ops: self.ops,
            bookmarks_added: self.added,
            bookmarks_deduped: self.deduped,
            bookmarks_refreshed: self.refreshed,
            folders_created: self.folders_created,
        }
    }
}
