//! mde-bookmarks — the pure model + CRDT for the mesh-synced **Bookmarks**
//! collection (BOOKMARKS-1; design: `docs/design/mesh-bookmarks.md`).
//!
//! One shared, mesh-wide bookmark tree (lock Q8) that every enrolled node edits
//! offline and converges without conflicts. This crate is the headless model
//! both the mackesd bookmarks worker (BOOKMARKS-2, persistence + Syncthing) and
//! the `Surface::Bookmarks` UI import — no Servo, no credentials, no I/O.
//!
//! The pieces:
//!
//!   * [`Bookmark`] / [`Folder`] / [`Item`] — the converged **strict one-parent
//!     tree** (locks Q2, Q7): UUID ids minted at creation (lock Q1), a
//!     fractional-index [`order_key`](Bookmark::order_key) for manual order, a
//!     content-addressed [`favicon_ref`](Bookmark::favicon_ref) (lock Q6), and
//!     `modified` + `last_author` attribution (lock Q64) (model.rs).
//!   * [`Hlc`] / [`HlcClock`] / [`Author`] — the **Hybrid Logical Clock** that
//!     stamps every op with a mesh-total order (lock Q5) plus the user+node an
//!     op is attributed to (lock Q64) (hlc.rs).
//!   * [`key_between`] — **LSEQ-style fractional indexing**: mint an order key
//!     strictly between two siblings so a drag is one op, not a renumber storm
//!     (lock Q3) (order.rs).
//!   * [`Op`] / [`OpKind`] — the append-only op set (`AddBookmark`,
//!     `EditBookmark`, `MoveItem`, `DeleteItem`, `AddFolder`, `RenameFolder`),
//!     each carrying its [`Hlc`] + [`Author`] (op.rs).
//!   * [`Collection`] — the **LWW CRDT merge**: fold an op set (in any order)
//!     into a converged tree; LWW on every field **including delete** — no
//!     tombstones, a later HLC wins, a delete competes as an op (lock Q4)
//!     (crdt.rs).
//!   * [`FaviconStore`] — the **content-addressed, deduped** favicon blob store
//!     (lock Q6), a grow-only conflict-free map (favicon.rs).
//!   * [`import_file`] / [`plan_import`] — the **browser importers** (BOOKMARKS-3):
//!     Firefox `places.sqlite` (bookmarks only, read-only + immutable), Chromium
//!     `Bookmarks` JSON, and universal Netscape HTML (Safari via export). They
//!     parse each format into the model via the existing CRDT ops, dedup by
//!     normalized URL, and re-import idempotently under `Imported/<Browser>`.
//!     **Never read logins/cookies/history** — bookmarks only (import/).
//!
//! **No CRDT/mesh I/O**: no Servo, no Syncthing, no Bus, no wall clock, no
//! credentials — the live plumbing is the BOOKMARKS-2 worker's. The only I/O in
//! the crate is the BOOKMARKS-3 importers reading local browser export files
//! read-only. Services tier: no desktop-shell dep (the layered-tiers gate).

#![forbid(unsafe_code)]

mod crdt;
mod favicon;
mod hlc;
mod import;
mod model;
mod op;
mod order;

pub use crdt::{Collection, ItemState, Reg};
pub use favicon::{hash_bytes, FaviconStore};
pub use hlc::{Author, Hlc, HlcClock, NodeId, UserId};
pub use import::{
    detect_format, import_file, import_file_as, normalize_url, parse_file, plan_import,
    scan_profiles, ImportCandidate, ImportError, ImportFormat, ImportOutcome, ParsedBookmark,
    ParsedNode, ParsedTree,
};
pub use model::{Bookmark, ContentHash, Folder, Item, ItemKind, Source};
pub use op::{Edit, Op, OpKind};
pub use order::key_between;
