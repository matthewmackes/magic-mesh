//! The render-agnostic Bookmarks manager model (BOOKMARKS-4).
//!
//! This is the part of the surface that holds no egui at all: a state machine
//! over `mde-bookmarks`' pure [`Collection`] + CRDT [`Op`] set. The egui view
//! ([`crate::view`]) reads this model and turns it into widgets; everything
//! decision-shaped — which folder the list shows, the multi-selection, the live
//! search + sort, the compose/rename/confirm drafts, and how a drag reorder/move
//! resolves into a fractional-index key — lives here so it can be unit-tested
//! without a GPU or a display.
//!
//! The reuse is deliberate (governance §6): **every** mutation mints a real
//! [`Op`] (stamped by an [`HlcClock`], attributed to an [`Author`]) and applies
//! it to the [`Collection`] — the exact ops the mackesd bookmarks worker
//! (BOOKMARKS-2) will replay + Syncthing-sync. This surface never re-implements
//! the tree; it drives the model's ops and reads back the converged tree.
//!
//! **Seams, honestly labelled.** Persistence + mesh sync are the BOOKMARKS-2
//! worker's job — this model edits an in-memory [`Collection`] and exposes
//! [`Manager::from_collection`] so a future worker binding hands its converged
//! collection straight in (no stub, a real constructor). "Open in a browser tab"
//! is the Servo browser (BOOKMARKS-5/6): [`Manager::open`] records the intent and
//! the detail pane surfaces it under an honest browser seam — it never fakes a
//! browser (§7).

use std::collections::{BTreeSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use mde_bookmarks::{
    key_between, Author, Collection, Folder, HlcClock, Item, ItemKind, Op, OpKind, Source,
};

/// How the list orders its rows (lock Q31). [`SortBy::Manual`] is the default —
/// the fractional-index drag order; the rest are view-only re-sorts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortBy {
    /// Manual fractional-index order (drag order) — the default.
    #[default]
    Manual,
    /// Alphabetical by display title / folder name.
    Title,
    /// Alphabetical by URL (folders fall back to their name).
    Url,
    /// Oldest-added first.
    Added,
    /// Most-recently-edited first.
    Recent,
}

impl SortBy {
    /// The fixed set of sort choices, in menu order.
    pub const ALL: [Self; 5] = [
        Self::Manual,
        Self::Title,
        Self::Url,
        Self::Added,
        Self::Recent,
    ];

    /// The menu label for this choice.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manual order",
            Self::Title => "Title",
            Self::Url => "URL",
            Self::Added => "Date added",
            Self::Recent => "Recently edited",
        }
    }
}

/// The outcome of the most-recent user action, shown in the honest status line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ActionOutcome {
    /// Nothing done yet this session.
    #[default]
    Idle,
    /// A neutral note (an honest seam, e.g. the browser-tab intent).
    Note(String),
    /// A completed edit (rendered in the success tone).
    Done(String),
}

/// The whole render-agnostic state of the Bookmarks surface.
pub struct Manager {
    /// The converged bookmark tree. Every edit applies one [`Op`] to it.
    collection: Collection,
    /// The Hybrid Logical Clock that stamps each locally-authored op (lock Q5).
    clock: HlcClock,
    /// The local author every op is attributed to (lock Q64). Best-effort local
    /// identity here; the authoritative user/node binding is the worker's.
    author: Author,

    /// The folder whose children the list shows (`None` = the collection root).
    current: Option<Uuid>,
    /// The expanded folders in the tree (persist across frames).
    expanded: BTreeSet<Uuid>,
    /// The multi-selection (Ctrl/Shift), by item id (lock Q32).
    selection: BTreeSet<Uuid>,
    /// The Shift-range anchor — the last single-selected id.
    anchor: Option<Uuid>,
    /// The item shown in the detail pane (`None` = nothing focused).
    detail: Option<Uuid>,
    /// URLs the user asked to "open" — surfaced under the browser seam until the
    /// Servo browser (BOOKMARKS-5/6) lands.
    open_intent: Vec<String>,

    /// The live title+URL search query (lock Q27); empty = browse `current`.
    query: String,
    /// The list sort order (lock Q31).
    sort: SortBy,
    /// The last action outcome (status line).
    last: ActionOutcome,

    // ── Compose / dialog drafts (render-agnostic; the view binds text fields) ──
    /// Whether the add-bookmark form is open.
    add_open: bool,
    /// The add form's URL field.
    add_url: String,
    /// The add form's title field (optional; derived from the URL when blank).
    add_title: String,
    /// The item being renamed inline, if any.
    rename_target: Option<Uuid>,
    /// The rename field buffer.
    rename_buf: String,
    /// A non-empty folder delete awaiting confirmation (lock Q30).
    confirm_delete: Option<Uuid>,
}

impl Manager {
    /// Build a manager over an empty collection, authored as `author`.
    #[must_use]
    pub fn new(author: Author) -> Self {
        Self::from_collection(Collection::new(), author)
    }

    /// Build a manager over an existing converged `collection` — the seam the
    /// BOOKMARKS-2 worker binds to (it hands its replayed+merged collection
    /// straight in). The clock is keyed to the author's node so locally-minted
    /// stamps stay node-attributed.
    #[must_use]
    pub fn from_collection(collection: Collection, author: Author) -> Self {
        let clock = HlcClock::new(author.node.clone());
        Self {
            collection,
            clock,
            author,
            current: None,
            expanded: BTreeSet::new(),
            selection: BTreeSet::new(),
            anchor: None,
            detail: None,
            open_intent: Vec::new(),
            query: String::new(),
            sort: SortBy::Manual,
            last: ActionOutcome::Idle,
            add_open: false,
            add_url: String::new(),
            add_title: String::new(),
            rename_target: None,
            rename_buf: String::new(),
            confirm_delete: None,
        }
    }

    /// Build a manager under the best-effort **local** identity — the OS user +
    /// hostname — for the standalone surface. This is the honest local author;
    /// the authoritative mesh user/node binding is the worker's (lock Q64).
    #[must_use]
    pub fn local() -> Self {
        let user = env_first(&["USER", "LOGNAME"]).unwrap_or_else(|| "user".to_string());
        let node = env_first(&["HOSTNAME", "HOST"]).unwrap_or_else(|| "node".to_string());
        Self::new(Author::new(user, node))
    }

    // ── Op plumbing (§6 — the one path every edit takes) ─────────────────────

    /// Mint one op (stamped + attributed) and apply it to the collection — the
    /// single glue point over `mde-bookmarks` every edit routes through.
    fn commit(&mut self, kind: OpKind) {
        let hlc = self.clock.tick(now_ms());
        let op = Op::new(hlc, self.author.clone(), kind);
        self.collection.apply(&op);
    }

    // ── Reads ────────────────────────────────────────────────────────────────

    /// The local author every op is attributed to.
    #[must_use]
    pub const fn author(&self) -> &Author {
        &self.author
    }

    /// Whether the whole collection is empty (the honest first-run state).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.collection.is_empty()
    }

    /// The number of live items across the whole collection.
    #[must_use]
    pub fn total(&self) -> usize {
        self.collection.len()
    }

    /// The folder whose children the list shows (`None` = root).
    #[must_use]
    pub const fn current(&self) -> Option<Uuid> {
        self.current
    }

    /// The converged [`Item`] for `id`, if it is live.
    #[must_use]
    pub fn item(&self, id: Uuid) -> Option<Item> {
        self.collection.item(id)
    }

    /// The converged [`Folder`] for `id`, if it is a live folder.
    #[must_use]
    pub fn folder(&self, id: Uuid) -> Option<Folder> {
        match self.collection.item(id) {
            Some(Item::Folder(f)) => Some(f),
            _ => None,
        }
    }

    /// The live child **folders** of `parent`, in render order — the tree rows.
    #[must_use]
    pub fn child_folders(&self, parent: Option<Uuid>) -> Vec<Folder> {
        self.collection
            .children(parent)
            .into_iter()
            .filter_map(|it| match it {
                Item::Folder(f) => Some(f),
                Item::Bookmark(_) => None,
            })
            .collect()
    }

    /// Whether `id` has any live child folder (drives the tree's expander).
    #[must_use]
    pub fn has_child_folders(&self, id: Uuid) -> bool {
        !self.child_folders(Some(id)).is_empty()
    }

    /// Whether the tree folder `id` is expanded.
    #[must_use]
    pub fn is_expanded(&self, id: Uuid) -> bool {
        self.expanded.contains(&id)
    }

    /// Toggle a tree folder's expansion.
    pub fn toggle_expanded(&mut self, id: Uuid) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
    }

    /// The root→current folder chain (the list breadcrumb).
    #[must_use]
    pub fn breadcrumb(&self) -> Vec<Folder> {
        let mut chain = Vec::new();
        let mut cursor = self.current;
        while let Some(id) = cursor {
            match self.folder(id) {
                Some(f) => {
                    cursor = f.parent;
                    chain.push(f);
                }
                None => break,
            }
        }
        chain.reverse();
        chain
    }

    /// The list rows: the live search hits across the whole tree when a query is
    /// set (lock Q27), otherwise `current`'s children — both in the chosen sort.
    #[must_use]
    pub fn listing(&self) -> Vec<Item> {
        let mut items = if self.query.trim().is_empty() {
            self.collection.children(self.current)
        } else {
            let q = self.query.to_lowercase();
            self.collection
                .items()
                .into_iter()
                .filter(|it| match it {
                    Item::Bookmark(b) => {
                        b.title.to_lowercase().contains(&q) || b.url.to_lowercase().contains(&q)
                    }
                    Item::Folder(_) => false,
                })
                .collect()
        };
        self.sort_items(&mut items);
        items
    }

    /// The ids of the current listing, in render order — the order Shift-range
    /// selection and multi-drag placement resolve against.
    #[must_use]
    pub fn listing_ids(&self) -> Vec<Uuid> {
        self.listing().iter().map(Item::id).collect()
    }

    /// Order `items` in place by the active [`SortBy`].
    fn sort_items(&self, items: &mut [Item]) {
        match self.sort {
            SortBy::Manual => items.sort_by(|a, b| {
                a.order_key()
                    .cmp(b.order_key())
                    .then_with(|| a.id().cmp(&b.id()))
            }),
            SortBy::Title => items.sort_by(|a, b| {
                display_label(a)
                    .to_lowercase()
                    .cmp(&display_label(b).to_lowercase())
            }),
            SortBy::Url => items.sort_by_cached_key(sort_url),
            SortBy::Added => items.sort_by_key(added_ms),
            SortBy::Recent => items.sort_by_key(|it| std::cmp::Reverse(modified_ms(it))),
        }
    }

    // ── Navigation ───────────────────────────────────────────────────────────

    /// Focus a folder's contents in the list (`None` = root). Clears any search
    /// so the browsed folder is what shows.
    pub fn open_folder(&mut self, folder: Option<Uuid>) {
        self.current = folder;
        self.query.clear();
        self.clear_selection();
    }

    /// Navigate up one level from `current` toward the root.
    pub fn go_up(&mut self) {
        let parent = self
            .current
            .and_then(|id| self.folder(id))
            .and_then(|f| f.parent);
        self.open_folder(parent);
    }

    // ── Search / sort ────────────────────────────────────────────────────────

    /// The live search query.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// A mutable handle on the search field, for the view's text binding.
    pub const fn query_mut(&mut self) -> &mut String {
        &mut self.query
    }

    /// Whether a search is active (the list shows hits, not a folder).
    #[must_use]
    pub fn is_searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// The active sort order.
    #[must_use]
    pub const fn sort(&self) -> SortBy {
        self.sort
    }

    /// Choose the list sort order.
    pub const fn set_sort(&mut self, sort: SortBy) {
        self.sort = sort;
    }

    // ── Selection (lock Q32) ─────────────────────────────────────────────────

    /// The current multi-selection.
    #[must_use]
    pub const fn selection(&self) -> &BTreeSet<Uuid> {
        &self.selection
    }

    /// Whether `id` is selected.
    #[must_use]
    pub fn is_selected(&self, id: Uuid) -> bool {
        self.selection.contains(&id)
    }

    /// The number of selected items.
    #[must_use]
    pub fn selection_len(&self) -> usize {
        self.selection.len()
    }

    /// Plain click: select just `id`, focus it in the detail pane, and set it as
    /// the Shift-range anchor.
    pub fn select_only(&mut self, id: Uuid) {
        self.selection.clear();
        self.selection.insert(id);
        self.anchor = Some(id);
        self.detail = Some(id);
    }

    /// Ctrl-click: toggle `id` in the selection (and move the anchor onto it).
    pub fn select_toggle(&mut self, id: Uuid) {
        if !self.selection.remove(&id) {
            self.selection.insert(id);
            self.detail = Some(id);
        }
        self.anchor = Some(id);
    }

    /// Shift-click: select the contiguous range from the anchor to `id` within
    /// the current listing order. With no anchor it degrades to a single select.
    pub fn select_range_to(&mut self, id: Uuid) {
        let order = self.listing_ids();
        let Some(anchor) = self.anchor else {
            self.select_only(id);
            return;
        };
        let (Some(a), Some(b)) = (
            order.iter().position(|x| *x == anchor),
            order.iter().position(|x| *x == id),
        ) else {
            self.select_only(id);
            return;
        };
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        self.selection.clear();
        for oid in &order[lo..=hi] {
            self.selection.insert(*oid);
        }
        self.detail = Some(id);
    }

    /// Clear the selection (keeps the detail focus).
    pub fn clear_selection(&mut self) {
        self.selection.clear();
        self.anchor = None;
    }

    // ── Detail pane / open intent ────────────────────────────────────────────

    /// The item focused in the detail pane, if any.
    #[must_use]
    pub fn detail(&self) -> Option<Item> {
        self.detail.and_then(|id| self.collection.item(id))
    }

    /// The URLs the user asked to open, awaiting the browser (BOOKMARKS-5/6).
    #[must_use]
    pub fn open_intent(&self) -> &[String] {
        &self.open_intent
    }

    /// Double-click / Enter on a list row. A folder navigates into itself; a
    /// bookmark records an open intent (lock Q79 — "double-click → new browser
    /// tab") and focuses the detail pane. The browser is BOOKMARKS-5/6, so the
    /// intent is surfaced honestly under the detail-pane seam, never faked.
    pub fn open(&mut self, id: Uuid) {
        match self.collection.item(id) {
            Some(Item::Folder(_)) => self.open_folder(Some(id)),
            Some(Item::Bookmark(b)) => {
                self.detail = Some(id);
                self.open_intent = vec![b.url.clone()];
                self.last = ActionOutcome::Note(format!(
                    "Opening {} in a browser tab arrives with the browser (BOOKMARKS-5).",
                    b.title
                ));
            }
            None => {}
        }
    }

    /// Bulk "open all" (lock Q32): record every selected bookmark's URL as an
    /// open intent. Same honest browser seam as [`Manager::open`].
    pub fn open_selection(&mut self) {
        let urls = self.selected_urls();
        let count = urls.len();
        self.open_intent = urls;
        if count > 0 {
            self.last = ActionOutcome::Note(format!(
                "Opening {count} bookmark(s) in browser tabs arrives with the browser (BOOKMARKS-5)."
            ));
        }
    }

    /// The URLs of the selected bookmarks, in listing order (folders skipped).
    #[must_use]
    pub fn selected_urls(&self) -> Vec<String> {
        self.listing()
            .into_iter()
            .filter(|it| self.selection.contains(&it.id()))
            .filter_map(|it| match it {
                Item::Bookmark(b) => Some(b.url),
                Item::Folder(_) => None,
            })
            .collect()
    }

    /// Bulk "copy URLs" (lock Q32): the selected bookmarks' URLs, newline-joined,
    /// for the view to put on the clipboard. Records the honest outcome.
    pub fn copy_selected_urls(&mut self) -> String {
        let urls = self.selected_urls();
        let joined = urls.join("\n");
        self.last = match urls.len() {
            0 => ActionOutcome::Note("No bookmark URLs in the selection to copy.".to_string()),
            n => ActionOutcome::Done(format!("Copied {n} URL(s) to the clipboard.")),
        };
        joined
    }

    /// The URL of a single bookmark, for the detail pane's "Copy URL" — the view
    /// puts the returned string on the clipboard. Records the honest outcome and
    /// returns an empty string for a folder (nothing to copy).
    pub fn copy_url(&mut self, id: Uuid) -> String {
        if let Some(Item::Bookmark(b)) = self.collection.item(id) {
            self.last = ActionOutcome::Done("Copied URL to the clipboard.".to_string());
            b.url
        } else {
            self.last = ActionOutcome::Note("Only bookmarks have a URL to copy.".to_string());
            String::new()
        }
    }

    /// The last action outcome (status line).
    #[must_use]
    pub const fn last_action(&self) -> &ActionOutcome {
        &self.last
    }

    // ── Add (lock Q26) ───────────────────────────────────────────────────────

    /// Whether the add-bookmark form is open.
    #[must_use]
    pub const fn add_open(&self) -> bool {
        self.add_open
    }

    /// Open the add form (the `+` / keyboard-shortcut entry point).
    pub const fn open_add(&mut self) {
        self.add_open = true;
    }

    /// Open the add form pre-filled from a pasted URL (lock Q26 — paste path).
    pub fn open_add_with_url(&mut self, url: impl Into<String>) {
        self.add_url = url.into();
        self.add_title.clear();
        self.add_open = true;
    }

    /// Close and reset the add form.
    pub fn cancel_add(&mut self) {
        self.add_open = false;
        self.add_url.clear();
        self.add_title.clear();
    }

    /// A mutable handle on the add form's URL field.
    pub const fn add_url_mut(&mut self) -> &mut String {
        &mut self.add_url
    }

    /// A mutable handle on the add form's title field.
    pub const fn add_title_mut(&mut self) -> &mut String {
        &mut self.add_title
    }

    /// Whether the add form can be submitted (a non-blank URL).
    #[must_use]
    pub fn can_submit_add(&self) -> bool {
        !self.add_url.trim().is_empty()
    }

    /// Commit the add form: mint an `AddBookmark` into `current` (title derived
    /// from the URL host when blank), then reset the form. A no-op on a blank URL.
    pub fn commit_add(&mut self) {
        if !self.can_submit_add() {
            return;
        }
        let url = self.add_url.trim().to_string();
        let title = if self.add_title.trim().is_empty() {
            title_from_url(&url)
        } else {
            self.add_title.trim().to_string()
        };
        let id = self.add_bookmark(url, title, self.current);
        self.cancel_add();
        self.select_only(id);
    }

    /// Add a bookmark leaf at the tail of `parent`'s children (lock Q1 mints the
    /// id, lock Q3 the order key). Returns the new id.
    pub fn add_bookmark(
        &mut self,
        url: impl Into<String>,
        title: impl Into<String>,
        parent: Option<Uuid>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        let order_key = self.tail_key(parent);
        let added = now_ms();
        let title = title.into();
        self.commit(OpKind::AddBookmark {
            id,
            parent,
            order_key,
            url: url.into(),
            title: title.clone(),
            favicon_ref: None,
            tags: Vec::new(),
            notes: String::new(),
            added,
            source: Source::Manual,
        });
        self.last = ActionOutcome::Done(format!("Added \u{201c}{title}\u{201d}."));
        id
    }

    // ── Folder CRUD (lock Q30) ───────────────────────────────────────────────

    /// Create a folder at the tail of `parent`'s children. Returns the new id.
    pub fn add_folder(&mut self, name: impl Into<String>, parent: Option<Uuid>) -> Uuid {
        let id = Uuid::new_v4();
        let order_key = self.tail_key(parent);
        let name = name.into();
        self.commit(OpKind::AddFolder {
            id,
            name: name.clone(),
            parent,
            order_key,
        });
        if let Some(p) = parent {
            self.expanded.insert(p);
        }
        self.last = ActionOutcome::Done(format!("Created folder \u{201c}{name}\u{201d}."));
        id
    }

    /// The number of live descendants of `id` (drives confirm-on-non-empty).
    #[must_use]
    pub fn descendant_count(&self, id: Uuid) -> usize {
        self.subtree(id).len().saturating_sub(1)
    }

    // ── Rename (folders + bookmark titles) ───────────────────────────────────

    /// The item currently being renamed inline, if any.
    #[must_use]
    pub const fn rename_target(&self) -> Option<Uuid> {
        self.rename_target
    }

    /// A mutable handle on the rename buffer, for the view's text binding.
    pub const fn rename_buf_mut(&mut self) -> &mut String {
        &mut self.rename_buf
    }

    /// Begin renaming `id`, seeding the buffer with its current name/title.
    pub fn begin_rename(&mut self, id: Uuid) {
        self.rename_buf = self
            .item(id)
            .as_ref()
            .map(display_label)
            .unwrap_or_default();
        self.rename_target = Some(id);
    }

    /// Cancel an in-progress rename.
    pub fn cancel_rename(&mut self) {
        self.rename_target = None;
        self.rename_buf.clear();
    }

    /// Commit the inline rename: a folder gets a `RenameFolder`, a bookmark an
    /// `EditBookmark { title }`. A blank name is ignored (kept honest — no empty
    /// titles). A no-op when nothing is being renamed.
    pub fn commit_rename(&mut self) {
        let Some(id) = self.rename_target else {
            return;
        };
        let name = self.rename_buf.trim().to_string();
        if name.is_empty() {
            self.cancel_rename();
            return;
        }
        match self.collection.item(id) {
            Some(Item::Folder(_)) => self.commit(OpKind::RenameFolder {
                id,
                name: name.clone(),
            }),
            Some(Item::Bookmark(_)) => self.commit(OpKind::EditBookmark {
                id,
                url: None,
                title: Some(name.clone()),
                favicon_ref: None,
                tags: None,
                notes: None,
            }),
            None => {}
        }
        self.cancel_rename();
        self.last = ActionOutcome::Done(format!("Renamed to \u{201c}{name}\u{201d}."));
    }

    // ── Delete (lock Q30 confirm-on-non-empty) ───────────────────────────────

    /// The folder whose non-empty delete is awaiting confirmation, if any.
    #[must_use]
    pub const fn confirm_delete(&self) -> Option<Uuid> {
        self.confirm_delete
    }

    /// Request deleting `id`. A bookmark or an empty folder deletes immediately;
    /// a non-empty folder parks for confirmation (lock Q30).
    pub fn request_delete(&mut self, id: Uuid) {
        let is_folder = matches!(self.collection.item(id), Some(Item::Folder(_)));
        if is_folder && self.descendant_count(id) > 0 {
            self.confirm_delete = Some(id);
        } else {
            self.delete_subtree(id);
        }
    }

    /// Confirm the parked non-empty folder delete.
    pub fn confirm_delete_yes(&mut self) {
        if let Some(id) = self.confirm_delete.take() {
            self.delete_subtree(id);
        }
    }

    /// Dismiss the parked delete without deleting.
    pub const fn confirm_delete_no(&mut self) {
        self.confirm_delete = None;
    }

    /// Delete `id` and its whole subtree — each item its own `DeleteItem` op
    /// (lock Q4: a delete is an LWW op, not a tombstone), so a folder never
    /// leaves orphaned descendants. Prunes the id from selection/detail/expanded.
    pub fn delete_subtree(&mut self, id: Uuid) {
        let label = self
            .item(id)
            .as_ref()
            .map(display_label)
            .unwrap_or_default();
        let victims = self.subtree(id);
        for vid in &victims {
            self.commit(OpKind::DeleteItem { id: *vid });
            self.selection.remove(vid);
            self.expanded.remove(vid);
            if self.detail == Some(*vid) {
                self.detail = None;
            }
            if self.current == Some(*vid) {
                self.current = None;
            }
        }
        self.last = ActionOutcome::Done(format!("Deleted \u{201c}{label}\u{201d}."));
    }

    // ── Bulk (lock Q32) ──────────────────────────────────────────────────────

    /// Bulk-delete the whole selection (each subtree). Skips confirmation — the
    /// view gates the bulk bar behind its own confirm.
    pub fn bulk_delete(&mut self) {
        let roots = self.ordered_selection();
        let count = roots.len();
        for id in roots {
            // Some ids may already have vanished as a descendant of an earlier
            // deletion this pass — deleting a gone id is a harmless no-op.
            if self.collection.item(id).is_some() {
                let victims = self.subtree(id);
                for vid in victims {
                    self.commit(OpKind::DeleteItem { id: vid });
                    self.expanded.remove(&vid);
                    if self.detail == Some(vid) {
                        self.detail = None;
                    }
                }
            }
        }
        self.clear_selection();
        self.last = ActionOutcome::Done(format!("Deleted {count} item(s)."));
    }

    /// Bulk-move the selection into `folder` (`None` = root), appending at the
    /// tail. Cycle-forming moves (a folder into its own descendant) are skipped.
    pub fn bulk_move(&mut self, folder: Option<Uuid>) {
        let ids: Vec<Uuid> = self.ordered_selection();
        self.move_into(&ids, folder);
        let dest = folder.and_then(|id| self.folder(id)).map_or_else(
            || "the top level".to_string(),
            |f| format!("\u{201c}{}\u{201d}", f.name),
        );
        self.last = ActionOutcome::Done(format!("Moved {} item(s) to {dest}.", ids.len()));
    }

    // ── Drag reorder / move (lock Q29) ───────────────────────────────────────

    /// The batch a drag of `grabbed` carries: the whole selection (in listing
    /// order) when `grabbed` is part of a multi-selection, else just `grabbed`.
    #[must_use]
    pub fn drag_batch(&self, grabbed: Uuid) -> Vec<Uuid> {
        if self.selection.len() > 1 && self.selection.contains(&grabbed) {
            self.ordered_selection()
        } else {
            vec![grabbed]
        }
    }

    /// Move `ids` to the tail of `folder`'s children (a drag-onto-folder move).
    pub fn move_into(&mut self, ids: &[Uuid], folder: Option<Uuid>) {
        for id in ids {
            if self.can_move(*id, folder) {
                let key = self.tail_key(folder);
                self.commit(OpKind::MoveItem {
                    id: *id,
                    parent: folder,
                    order_key: key,
                });
            }
        }
    }

    /// Reorder `ids` to land immediately before `target` (a drag-onto-row
    /// reorder), reparenting into `target`'s folder. Preserves the batch's
    /// relative order; skips any id that would form a cycle or is `target`.
    pub fn move_before(&mut self, ids: &[Uuid], target: Uuid) {
        let Some(target_item) = self.collection.item(target) else {
            return;
        };
        let parent = target_item.parent();
        let siblings = self.collection.children(parent);
        let Some(pos) = siblings.iter().position(|it| it.id() == target) else {
            return;
        };
        // The key just below the target — the lower bound the batch wedges above.
        let mut lo = if pos == 0 {
            None
        } else {
            Some(siblings[pos - 1].order_key().to_string())
        };
        let hi = target_item.order_key().to_string();
        for id in ids {
            if *id == target || !self.can_move(*id, parent) {
                continue;
            }
            let key = key_between(lo.as_deref(), Some(&hi));
            self.commit(OpKind::MoveItem {
                id: *id,
                parent,
                order_key: key.clone(),
            });
            lo = Some(key);
        }
    }

    /// Whether moving `id` under `new_parent` is legal — never into itself or one
    /// of its own descendants (a folder cycle would corrupt the tree).
    #[must_use]
    fn can_move(&self, id: Uuid, new_parent: Option<Uuid>) -> bool {
        match new_parent {
            None => true,
            Some(p) if p == id => false,
            Some(p) => !self.subtree(id).contains(&p),
        }
    }

    // ── Internal tree helpers ────────────────────────────────────────────────

    /// `id` plus every live descendant (BFS), for subtree delete / cycle checks.
    fn subtree(&self, id: Uuid) -> Vec<Uuid> {
        let mut out = Vec::new();
        let mut queue = VecDeque::from([id]);
        while let Some(cur) = queue.pop_front() {
            if self.collection.item(cur).is_none() || out.contains(&cur) {
                continue;
            }
            out.push(cur);
            for child in self.collection.children(Some(cur)) {
                queue.push_back(child.id());
            }
        }
        out
    }

    /// The selection ordered by the current listing (stable batch placement).
    fn ordered_selection(&self) -> Vec<Uuid> {
        let mut ids: Vec<Uuid> = self
            .listing_ids()
            .into_iter()
            .filter(|id| self.selection.contains(id))
            .collect();
        // Anything selected but not in the current listing (e.g. selected, then
        // navigated) still gets moved — appended in id order.
        for id in &self.selection {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
        ids
    }

    /// The order key one step past the last child of `parent` (append at tail).
    fn tail_key(&self, parent: Option<Uuid>) -> String {
        let last = self
            .collection
            .children(parent)
            .last()
            .map(|it| it.order_key().to_string());
        key_between(last.as_deref(), None)
    }
}

// ── Free helpers (pure; unit-tested) ─────────────────────────────────────────

/// The display label of an item — a bookmark's title (its URL when the title is
/// blank) or a folder's name.
#[must_use]
pub fn display_label(item: &Item) -> String {
    match item {
        Item::Bookmark(b) if b.title.is_empty() => b.url.clone(),
        Item::Bookmark(b) => b.title.clone(),
        Item::Folder(f) => f.name.clone(),
    }
}

/// The sort key for [`SortBy::Url`]: a bookmark's URL, or a folder's name (so
/// folders still order sensibly in a URL sort).
fn sort_url(item: &Item) -> String {
    match item {
        Item::Bookmark(b) => b.url.to_lowercase(),
        Item::Folder(f) => f.name.to_lowercase(),
    }
}

/// A bookmark's added time (folders have none → 0, so they group first).
const fn added_ms(item: &Item) -> u64 {
    match item {
        Item::Bookmark(b) => b.added,
        Item::Folder(_) => 0,
    }
}

/// A bookmark's last-modified time (folders → 0).
const fn modified_ms(item: &Item) -> u64 {
    match item {
        Item::Bookmark(b) => b.modified,
        Item::Folder(_) => 0,
    }
}

/// Derive a readable title from a URL host when the user left the title blank —
/// strip the scheme + `www.` and take the host segment. Falls back to the whole
/// URL when there is no recognizable host.
#[must_use]
pub fn title_from_url(url: &str) -> String {
    let no_scheme = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .trim_start_matches("www.");
    let host = no_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(no_scheme)
        .trim();
    if host.is_empty() {
        url.trim().to_string()
    } else {
        host.to_string()
    }
}

/// The current wall time in ms, injected into the HLC. The pure model never
/// reads the clock; this desktop-tier surface does, and the HLC keeps every stamp
/// monotonic even if the clock stalls or runs backwards.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(u64::MAX)
}

/// The first set, non-empty environment variable among `keys` — best-effort
/// local identity for the standalone surface.
fn env_first(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        std::env::var(k)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

/// The `ItemKind` of an item — a small convenience for the view.
#[must_use]
pub const fn kind_of(item: &Item) -> ItemKind {
    match item {
        Item::Bookmark(_) => ItemKind::Bookmark,
        Item::Folder(_) => ItemKind::Folder,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manager under a fixed test author — deterministic node attribution, no
    /// env reads. Every edit drives real `mde-bookmarks` ops, so these tests
    /// exercise the same CRDT path production does.
    fn manager() -> Manager {
        Manager::new(Author::new("tester".into(), "test-node".into()))
    }

    fn titles(items: &[Item]) -> Vec<String> {
        items.iter().map(display_label).collect()
    }

    #[test]
    fn starts_empty_and_honest() {
        let m = manager();
        assert!(m.is_empty());
        assert_eq!(m.total(), 0);
        assert!(m.listing().is_empty());
        assert!(m.detail().is_none());
        assert_eq!(*m.last_action(), ActionOutcome::Idle);
    }

    #[test]
    fn add_bookmark_appears_in_the_listing() {
        let mut m = manager();
        let id = m.add_bookmark("https://example.com", "Example", None);
        assert_eq!(m.total(), 1);
        assert_eq!(titles(&m.listing()), vec!["Example"]);
        assert!(matches!(m.item(id), Some(Item::Bookmark(_))));
        assert!(matches!(m.last_action(), ActionOutcome::Done(_)));
    }

    #[test]
    fn commit_add_derives_a_title_from_the_url_when_blank() {
        let mut m = manager();
        m.open_add();
        assert!(m.add_open());
        add_url(&mut m, "https://www.rust-lang.org/tools");
        assert!(m.can_submit_add());
        m.commit_add();
        assert!(!m.add_open(), "form closes on submit");
        assert_eq!(titles(&m.listing()), vec!["rust-lang.org"]);
    }

    #[test]
    fn blank_url_never_adds() {
        let mut m = manager();
        m.open_add();
        add_url(&mut m, "   ");
        assert!(!m.can_submit_add());
        m.commit_add();
        assert!(m.is_empty(), "a blank URL must never mint a bookmark");
    }

    #[test]
    fn folder_crud_and_navigation() {
        let mut m = manager();
        let work = m.add_folder("Work", None);
        assert!(m.folder(work).is_some());
        // A bookmark inside the folder.
        m.add_bookmark("https://intranet", "Intranet", Some(work));
        // Root listing shows the folder; the folder's listing shows the bookmark.
        assert_eq!(titles(&m.listing()), vec!["Work"]);
        m.open_folder(Some(work));
        assert_eq!(m.current(), Some(work));
        assert_eq!(titles(&m.listing()), vec!["Intranet"]);
        // Rename the folder through the inline-rename path.
        m.begin_rename(work);
        assert_eq!(m.rename_target(), Some(work));
        *m.rename_buf_mut() = "Office".to_string();
        m.commit_rename();
        assert_eq!(m.folder(work).expect("folder lives").name, "Office");
    }

    #[test]
    fn deleting_a_nonempty_folder_confirms_first_then_cascades() {
        let mut m = manager();
        let f = m.add_folder("Docs", None);
        let child = m.add_bookmark("https://a", "A", Some(f));
        assert_eq!(m.descendant_count(f), 1);
        // A non-empty folder parks for confirmation, not an immediate delete.
        m.request_delete(f);
        assert_eq!(m.confirm_delete(), Some(f));
        assert!(m.item(f).is_some(), "not deleted until confirmed");
        // Confirming cascades — folder AND child gone (no orphan).
        m.confirm_delete_yes();
        assert!(m.item(f).is_none());
        assert!(m.item(child).is_none(), "descendant deleted too");
        assert!(m.is_empty());
    }

    #[test]
    fn deleting_a_bookmark_is_immediate() {
        let mut m = manager();
        let id = m.add_bookmark("https://a", "A", None);
        m.request_delete(id);
        assert_eq!(m.confirm_delete(), None, "a leaf needs no confirmation");
        assert!(m.item(id).is_none());
    }

    #[test]
    fn live_search_matches_title_and_url_across_the_tree() {
        let mut m = manager();
        let f = m.add_folder("Nested", None);
        m.add_bookmark("https://rust-lang.org", "Rust", Some(f));
        m.add_bookmark("https://python.org", "Python", None);
        // From the root, nested items aren't listed…
        assert_eq!(titles(&m.listing()), vec!["Nested", "Python"]);
        // …but search reaches the whole tree, by title…
        *m.query_mut() = "rust".to_string();
        assert!(m.is_searching());
        assert_eq!(titles(&m.listing()), vec!["Rust"]);
        // …and by URL substring.
        *m.query_mut() = "python.org".to_string();
        assert_eq!(titles(&m.listing()), vec!["Python"]);
    }

    #[test]
    fn sort_by_title_orders_the_listing() {
        let mut m = manager();
        m.add_bookmark("https://c", "Charlie", None);
        m.add_bookmark("https://a", "Alpha", None);
        m.add_bookmark("https://b", "Bravo", None);
        // Manual (insertion/tail) order first.
        assert_eq!(titles(&m.listing()), vec!["Charlie", "Alpha", "Bravo"]);
        m.set_sort(SortBy::Title);
        assert_eq!(titles(&m.listing()), vec!["Alpha", "Bravo", "Charlie"]);
    }

    #[test]
    fn ctrl_and_shift_multi_select() {
        let mut m = manager();
        let a = m.add_bookmark("https://a", "A", None);
        let b = m.add_bookmark("https://b", "B", None);
        let c = m.add_bookmark("https://c", "C", None);
        // Plain click selects one and anchors it.
        m.select_only(a);
        assert_eq!(m.selection_len(), 1);
        // Ctrl-click adds a discontiguous one.
        m.select_toggle(c);
        assert!(m.is_selected(a) && m.is_selected(c) && !m.is_selected(b));
        // Ctrl-click again toggles it off.
        m.select_toggle(c);
        assert!(!m.is_selected(c));
        // Shift-click from the anchor (a) to c selects the whole range.
        m.select_only(a);
        m.select_range_to(c);
        assert_eq!(m.selection_len(), 3);
        assert!(m.is_selected(a) && m.is_selected(b) && m.is_selected(c));
    }

    #[test]
    fn bulk_copy_urls_joins_the_selection() {
        let mut m = manager();
        let a = m.add_bookmark("https://a.example", "A", None);
        let b = m.add_bookmark("https://b.example", "B", None);
        m.select_only(a);
        m.select_toggle(b);
        let copied = m.copy_selected_urls();
        assert_eq!(copied, "https://a.example\nhttps://b.example");
        assert!(matches!(m.last_action(), ActionOutcome::Done(_)));
    }

    #[test]
    fn bulk_move_reparents_the_selection() {
        let mut m = manager();
        let dest = m.add_folder("Dest", None);
        let a = m.add_bookmark("https://a", "A", None);
        let b = m.add_bookmark("https://b", "B", None);
        m.select_only(a);
        m.select_toggle(b);
        m.bulk_move(Some(dest));
        // Both now live under Dest; the root shows only the folder.
        assert_eq!(titles(&m.listing()), vec!["Dest"]);
        m.open_folder(Some(dest));
        assert_eq!(m.listing().len(), 2);
    }

    #[test]
    fn bulk_delete_removes_the_selection() {
        let mut m = manager();
        let a = m.add_bookmark("https://a", "A", None);
        let b = m.add_bookmark("https://b", "B", None);
        let _c = m.add_bookmark("https://c", "C", None);
        m.select_only(a);
        m.select_toggle(b);
        m.bulk_delete();
        assert_eq!(titles(&m.listing()), vec!["C"]);
        assert_eq!(m.selection_len(), 0);
    }

    #[test]
    fn drag_reorder_moves_before_the_target() {
        let mut m = manager();
        let a = m.add_bookmark("https://a", "A", None);
        let _b = m.add_bookmark("https://b", "B", None);
        let c = m.add_bookmark("https://c", "C", None);
        assert_eq!(titles(&m.listing()), vec!["A", "B", "C"]);
        // Drag C to before A → C, A, B.
        m.move_before(&[c], a);
        assert_eq!(titles(&m.listing()), vec!["C", "A", "B"]);
    }

    #[test]
    fn drag_onto_folder_moves_in() {
        let mut m = manager();
        let f = m.add_folder("F", None);
        let bm = m.add_bookmark("https://x", "X", None);
        m.move_into(&[bm], Some(f));
        // Root now shows just the folder; the bookmark moved inside.
        assert_eq!(titles(&m.listing()), vec!["F"]);
        m.open_folder(Some(f));
        assert_eq!(titles(&m.listing()), vec!["X"]);
    }

    #[test]
    fn a_folder_cannot_be_dragged_into_its_own_descendant() {
        let mut m = manager();
        let parent = m.add_folder("Parent", None);
        let child = m.add_folder("Child", Some(parent));
        // Moving Parent into Child would form a cycle — rejected, tree intact.
        m.move_into(&[parent], Some(child));
        assert_eq!(m.folder(parent).expect("parent lives").parent, None);
        assert_eq!(m.folder(child).expect("child lives").parent, Some(parent));
    }

    #[test]
    fn drag_batch_follows_the_multi_selection() {
        let mut m = manager();
        let a = m.add_bookmark("https://a", "A", None);
        let b = m.add_bookmark("https://b", "B", None);
        // Not multi-selected → the batch is just the grabbed id.
        assert_eq!(m.drag_batch(a), vec![a]);
        // Multi-selected + grabbing a member → the whole selection.
        m.select_only(a);
        m.select_toggle(b);
        assert_eq!(m.drag_batch(a), vec![a, b]);
    }

    #[test]
    fn double_click_bookmark_records_an_honest_open_intent() {
        let mut m = manager();
        let id = m.add_bookmark("https://site", "Site", None);
        m.open(id);
        assert_eq!(m.open_intent(), &["https://site".to_string()]);
        // The intent is an honest note about the browser seam, not a fake open.
        assert!(matches!(m.last_action(), ActionOutcome::Note(_)));
    }

    #[test]
    fn double_click_folder_navigates_in() {
        let mut m = manager();
        let f = m.add_folder("F", None);
        m.add_bookmark("https://in", "In", Some(f));
        m.open(f);
        assert_eq!(m.current(), Some(f));
        assert_eq!(titles(&m.listing()), vec!["In"]);
    }

    #[test]
    fn breadcrumb_tracks_the_current_folder_chain() {
        let mut m = manager();
        let a = m.add_folder("A", None);
        let b = m.add_folder("B", Some(a));
        m.open_folder(Some(b));
        let crumbs: Vec<String> = m.breadcrumb().into_iter().map(|f| f.name).collect();
        assert_eq!(crumbs, vec!["A", "B"]);
        m.go_up();
        assert_eq!(m.current(), Some(a));
    }

    #[test]
    fn title_from_url_strips_scheme_and_www() {
        assert_eq!(title_from_url("https://www.example.com/x?y"), "example.com");
        assert_eq!(title_from_url("http://host"), "host");
        assert_eq!(title_from_url("not a url"), "not a url");
    }

    /// Bind a URL into the add form the way the view's text field does.
    fn add_url(m: &mut Manager, url: &str) {
        m.add_url_mut().clear();
        m.add_url_mut().push_str(url);
    }
}
