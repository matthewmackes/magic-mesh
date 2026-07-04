//! The **CRDT buffer model** (EDITOR-COLLAB-1): the conflict-free replicated
//! document mesh co-editing is built on.
//!
//! Design: `docs/design/editor.md` Phase 3 — "Zed's multiplayer, but P2P over
//! the mesh". [`CollabDoc`] is glue over [`yrs`] — the pure-Rust port of Yjs (the YATA
//! text CRDT) — exactly as the worklist locks ("glue over an existing Rust
//! CRDT"). It is a **pure data structure**: no egui, no bus, no sockets. The
//! mesh share-session (EDITOR-COLLAB-2) carries its byte payloads over
//! mde-bus/Nebula; this unit is the replicated model + its convergence
//! guarantees, tested end-to-end in-process (§7 — real merges, not mocks).
//!
//! What it gives COLLAB-2:
//!
//! * **Local edits → broadcastable payloads** — [`CollabDoc::insert`] /
//!   [`CollabDoc::remove`] mutate the replicated text and return the compact
//!   binary (lib0 v1) update encoding of exactly that edit, ready to fan out to
//!   peers. Local-first: the local document is usable immediately; peers merge
//!   whenever the payload reaches them.
//! * **Remote merge** — [`CollabDoc::apply_remote`] integrates a peer's update
//!   (idempotent, order-tolerant: yrs parks an update whose causal
//!   predecessors have not arrived yet and integrates it when they do) and
//!   returns the [`TextEdit`] script that replays the change onto the editor's
//!   rope [`Buffer`] in char space.
//! * **The sync-protocol seam** — the y-sync state-vector handshake COLLAB-2
//!   speaks over the bus: a peer sends [`CollabDoc::encode_state_vector`]
//!   (a tiny per-client version summary), the other answers with
//!   [`CollabDoc::diff`] (an update containing **only the ops the requester is
//!   missing**, never the whole history), and the requester feeds that to
//!   [`apply_remote`](CollabDoc::apply_remote). Run in both directions it
//!   reconciles any two replicas — including after an offline batch — which is
//!   the whole reconnect story: no sequence numbers, no server.
//! * **The `Buffer` bridge** — [`CollabDoc::from_buffer`] /
//!   [`CollabDoc::to_text`] and the [`EditSink`] forwarding trait; the
//!   contract is documented on [`EditSink`] (and buffer.rs is untouched).
//!
//! # Convergence
//!
//! yrs guarantees strong eventual consistency: replicas that have seen the
//! same set of updates hold the same text, regardless of delivery order or
//! interleaving. Concurrent inserts at the same position are ordered
//! deterministically by YATA's tie-break on the (unique) client ids, so every
//! peer resolves a conflict to the **same** text without coordination. The
//! tests below exercise both delivery orders, same-position conflicts, a
//! 3-way merge, and an offline batch → reconnect handshake.
//!
//! # Index spaces (the one subtle contract)
//!
//! The editor's [`Buffer`] speaks **char** indices (`ropey`'s unicode-scalar
//! metric); yrs speaks **byte** offsets (the doc is built with
//! [`OffsetKind::Bytes`]). `CollabDoc` owns the conversion: its public API is
//! entirely char-indexed (matching `Buffer::insert`/`remove` argument-for-
//! argument), and an internal `ropey` mirror rope translates char → byte in
//! O(log n) per local edit. Remote applies re-materialize the yrs text and
//! diff it against the mirror (O(n) — fine for the model unit; COLLAB-2 can
//! upgrade to yrs observer deltas if profiling ever demands it).

// `module_name_repetitions`: `CrdtError` is the module's one error type;
// stripping the domain prefix (`Error`) would collide with `std::error::Error`
// in every importer. Same call buffer.rs makes for `Buffer`.
#![allow(clippy::module_name_repetitions)]

use std::fmt;
use std::ops::Range;

use ropey::Rope;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Doc, GetString, OffsetKind, Options, ReadTxn, StateVector, Text, TextRef, Transact, Update,
};

use crate::buffer::Buffer;

/// The yrs root type name the replicated text lives under. Every peer in a
/// session must agree on it (it is part of the document schema, not the wire
/// protocol), so it is a single shared constant rather than a parameter.
const TEXT_ROOT: &str = "content";

/// An error from decoding or integrating replicated bytes.
///
/// Both variants carry the underlying yrs error rendered to a string so the
/// share-session can surface it honestly (§7) without this module leaking yrs
/// error types into the crate's public API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CrdtError {
    /// The bytes were not a decodable lib0-v1 update / state vector (a
    /// truncated or corrupted frame off the bus).
    Decode(String),
    /// The update decoded but could not be integrated into the document.
    Apply(String),
}

impl fmt::Display for CrdtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(err) => write!(f, "undecodable CRDT payload: {err}"),
            Self::Apply(err) => write!(f, "CRDT update failed to integrate: {err}"),
        }
    }
}

impl std::error::Error for CrdtError {}

/// One char-space edit a remote merge performed, replayable onto a [`Buffer`].
///
/// [`CollabDoc::apply_remote`] returns a script of these describing how the
/// local text changed; applying them **in order** (via [`TextEdit::apply_to`])
/// brings the editor's rope buffer to the merged text. Positions are char
/// indices in the text state at that point of the script — exactly the
/// argument contract of [`Buffer::insert`] / [`Buffer::remove`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextEdit {
    /// `text` appears starting at char index `at`.
    Insert {
        /// Char index the inserted run starts at.
        at: usize,
        /// The inserted text.
        text: String,
    },
    /// The chars in `range` disappear.
    Remove {
        /// Char range removed (half-open, char indices).
        range: Range<usize>,
    },
}

impl TextEdit {
    /// Replay this edit onto `buffer` — the remote→[`Buffer`] leg of the
    /// bridge contract (see [`EditSink`]).
    ///
    /// # Panics
    /// Panics if the edit does not fit `buffer` (`ropey`'s contract) — i.e. if
    /// the buffer has drifted from the [`CollabDoc`] that produced the script.
    pub fn apply_to(&self, buffer: &mut Buffer) {
        match self {
            Self::Insert { at, text } => buffer.insert(*at, text),
            Self::Remove { range } => buffer.remove(range.clone()),
        }
    }
}

/// The [`Buffer`]→[`CollabDoc`] edit-forwarding seam: the **bridge contract**.
///
/// `Buffer` (buffer.rs, untouched by this unit) stays the editor's document
/// model; a live share session shadows it with a [`CollabDoc`]. The contract
/// that keeps the two in lockstep:
///
/// 1. **Seed once, one side.** The share **host** builds the doc from its
///    buffer ([`CollabDoc::from_buffer`]); every **guest** starts empty
///    ([`CollabDoc::new`]) and receives the content through the sync handshake
///    ([`CollabDoc::diff`] against its empty state vector). Two peers must
///    never both seed the same text — seeding creates ops, and duplicated seed
///    ops merge as duplicated text.
/// 2. **Forward every local mutation, same args, same order.** Wherever the
///    widget calls `buffer.insert(i, t)` / `buffer.remove(r)`, the session
///    wiring makes the twin [`EditSink::forward_insert`] /
///    [`EditSink::forward_remove`] call with the identical `(i, t)` / `(r)`
///    arguments. The positions agree because both sides speak char indices
///    over identical text.
/// 3. **Replay every remote merge.** Each script [`CollabDoc::apply_remote`]
///    returns is applied to the buffer in order via [`TextEdit::apply_to`]
///    before the next local edit is forwarded.
/// 4. **Solo undo/redo is suspended while shared.** [`Buffer::undo`]/`redo`
///    replay private history ops that are invisible to this seam (buffer.rs
///    exposes no op stream, and this unit does not modify it), so a live
///    session must route undo through the CRDT layer instead — collaborative
///    undo (yrs `UndoManager`: undo *my* edits without reverting peers') is
///    COLLAB-2 UI work. [`CollabDoc::in_sync_with`] is the cheap runtime
///    assert wiring can use to catch a contract breach honestly.
///
/// Under 1–3, `doc.to_text() == buffer text` holds at every quiescent point —
/// the round-trip the bridge test proves.
///
/// It is a trait (not inherent methods) so the panel wiring lands in COLLAB-2
/// against a seam: sessions can be absent (`Option<CollabDoc>`), faked in
/// widget tests, or wrapped (e.g. a recording sink) without touching this
/// module again.
pub trait EditSink {
    /// Mirror of [`Buffer::insert`]: `text` was inserted at char `char_idx`.
    fn forward_insert(&mut self, char_idx: usize, text: &str);
    /// Mirror of [`Buffer::remove`]: the chars in `range` were removed.
    fn forward_remove(&mut self, range: Range<usize>);
}

/// The conflict-free replicated document: a yrs text + the char↔byte mirror.
///
/// See the [module docs](self) for the full picture. One `CollabDoc` per
/// shared buffer per session; drop it when the session ends (the solo
/// [`Buffer`] remains the document of record throughout).
pub struct CollabDoc {
    /// The yrs document (owns the CRDT state; client id = this peer's site).
    doc: Doc,
    /// The one replicated text root (`TEXT_ROOT`) inside `doc`.
    text: TextRef,
    /// Char↔byte index mirror of the replicated text (`ropey`, O(log n)
    /// conversions). Invariant: always equals the yrs text.
    mirror: Rope,
    /// Updates queued by the [`EditSink`] forwarding calls, drained by
    /// [`Self::take_updates`] (the `Buffer::take_edits` idiom: mutate now,
    /// broadcast once per frame).
    outbox: Vec<Vec<u8>>,
}

impl CollabDoc {
    /// An empty replicated document for site `client_id`.
    ///
    /// `client_id` is this peer's YATA site id: it **must be unique among the
    /// session's peers** (yrs's convergence precondition) and is what makes
    /// same-position conflict resolution deterministic. COLLAB-2 derives it
    /// from the mesh node identity; tests pass small literals.
    #[must_use]
    pub fn new(client_id: u64) -> Self {
        let mut options = Options::with_client_id(client_id);
        // The mirror rope translates the editor's char indices to byte
        // offsets, so the yrs side must speak bytes (not the UTF-16 default
        // Yjs interop would pick).
        options.offset_kind = OffsetKind::Bytes;
        let doc = Doc::with_options(options);
        let text = doc.get_or_insert_text(TEXT_ROOT);
        Self {
            doc,
            text,
            mirror: Rope::new(),
            outbox: Vec::new(),
        }
    }

    /// A document seeded with `text` — the share **host** constructor (bridge
    /// contract rule 1: guests join empty and sync instead).
    #[must_use]
    pub fn from_text(text: &str, client_id: u64) -> Self {
        let mut this = Self::new(client_id);
        // The seed is initial state, not an edit to broadcast: guests receive
        // it through the state-vector handshake, so the payload is dropped.
        let _ = this.insert(0, text);
        this
    }

    /// A document seeded from the editor's rope `buffer` — the host side of
    /// the [`EditSink`] bridge contract. `doc.to_text()` equals the buffer's
    /// text from birth (the round-trip the bridge test proves).
    #[must_use]
    pub fn from_buffer(buffer: &Buffer, client_id: u64) -> Self {
        Self::from_text(&buffer.rope().to_string(), client_id)
    }

    /// This replica's YATA site id (unique per peer per session).
    #[must_use]
    pub fn client_id(&self) -> u64 {
        self.doc.client_id()
    }

    /// Total chars in the replicated text (the mirror's O(1) metric).
    #[must_use]
    pub fn len_chars(&self) -> usize {
        self.mirror.len_chars()
    }

    /// Materialize the replicated text — the [`Buffer`]-bound leg of the
    /// bridge (`to_text` in the contract). Reads the CRDT itself (the source
    /// of truth), not the mirror. O(n); callers keep incremental sync through
    /// the [`TextEdit`] scripts instead of re-materializing per frame.
    #[must_use]
    pub fn to_text(&self) -> String {
        self.text.get_string(&self.doc.transact())
    }

    /// Whether the replicated text currently equals `buffer`'s — the cheap
    /// runtime assert for the [`EditSink`] bridge contract (rule 4).
    #[must_use]
    pub fn in_sync_with(&self, buffer: &Buffer) -> bool {
        self.mirror == *buffer.rope()
    }

    /// Insert `text` at char `char_idx` (the twin of [`Buffer::insert`]) and
    /// return the binary update payload to broadcast, or `None` for the empty
    /// no-op (mirroring `Buffer`'s contract). Local-first: the local text is
    /// updated immediately; the payload reaches peers whenever it reaches
    /// them.
    ///
    /// # Panics
    /// Panics if `char_idx` is past the end of the text (`ropey`'s contract —
    /// the same contract as `Buffer::insert`), or if the document exceeds
    /// `u32::MAX` bytes (yrs's index width).
    #[must_use = "the update payload must reach peers (broadcast it) or they never see this edit"]
    pub fn insert(&mut self, char_idx: usize, text: &str) -> Option<Vec<u8>> {
        if text.is_empty() {
            return None;
        }
        let at = byte_index(&self.mirror, char_idx);
        let before = self.doc.transact().state_vector();
        {
            let mut txn = self.doc.transact_mut();
            self.text.insert(&mut txn, at, text);
        }
        self.mirror.insert(char_idx, text);
        Some(self.encode_since(&before))
    }

    /// Remove the chars in `range` (the twin of [`Buffer::remove`]) and return
    /// the binary update payload to broadcast, or `None` for an empty/reversed
    /// range (mirroring `Buffer`'s contract).
    ///
    /// # Panics
    /// Panics if `range` extends past the end of the text (`ropey`'s contract
    /// — the same contract as `Buffer::remove`), or if the document exceeds
    /// `u32::MAX` bytes (yrs's index width).
    #[must_use = "the update payload must reach peers (broadcast it) or they never see this edit"]
    pub fn remove(&mut self, range: Range<usize>) -> Option<Vec<u8>> {
        if range.start >= range.end {
            return None;
        }
        let start = byte_index(&self.mirror, range.start);
        let end = byte_index(&self.mirror, range.end);
        let before = self.doc.transact().state_vector();
        {
            let mut txn = self.doc.transact_mut();
            self.text.remove_range(&mut txn, start, end - start);
        }
        self.mirror.remove(range);
        Some(self.encode_since(&before))
    }

    /// Merge a peer's update into this replica and return the char-space
    /// [`TextEdit`] script that replays the change onto the editor's
    /// [`Buffer`] (empty when the update brought nothing new — re-delivery is
    /// harmless, which is what makes the transport idempotent).
    ///
    /// Order-tolerant: yrs parks an update whose causal predecessors have not
    /// arrived and integrates it once they do, so out-of-order delivery
    /// converges without any transport-level sequencing.
    ///
    /// # Errors
    /// [`CrdtError::Decode`] when `update` is not a lib0-v1 update frame;
    /// [`CrdtError::Apply`] when it decodes but cannot be integrated.
    pub fn apply_remote(&mut self, update: &[u8]) -> Result<Vec<TextEdit>, CrdtError> {
        let decoded =
            Update::decode_v1(update).map_err(|err| CrdtError::Decode(err.to_string()))?;
        {
            let mut txn = self.doc.transact_mut();
            txn.apply_update(decoded)
                .map_err(|err| CrdtError::Apply(err.to_string()))?;
        }
        let merged = self.text.get_string(&self.doc.transact());
        let before = self.mirror.to_string();
        let edits = char_span_diff(&before, &merged);
        if !edits.is_empty() {
            self.mirror = Rope::from_str(&merged);
        }
        Ok(edits)
    }

    /// This replica's encoded **state vector**: the tiny per-site version
    /// summary a peer answers with a [`diff`](Self::diff). One half of the
    /// y-sync handshake COLLAB-2 carries over mde-bus (module docs).
    #[must_use]
    pub fn encode_state_vector(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    /// The update containing **only the ops a peer at `remote_state_vector` is
    /// missing** — the other half of the y-sync handshake. Feeding the result
    /// to the peer's [`apply_remote`](Self::apply_remote) (and running the
    /// same exchange the other way) reconciles the two replicas, including
    /// after an offline batch; an already-synced peer gets a byte-sized empty
    /// update, never the history.
    ///
    /// # Errors
    /// [`CrdtError::Decode`] when the bytes are not an encoded state vector.
    pub fn diff(&self, remote_state_vector: &[u8]) -> Result<Vec<u8>, CrdtError> {
        let sv = StateVector::decode_v1(remote_state_vector)
            .map_err(|err| CrdtError::Decode(err.to_string()))?;
        Ok(self.doc.transact().encode_state_as_update_v1(&sv))
    }

    /// The whole document as one update (a diff against the empty state
    /// vector) — the session-bootstrap payload for a brand-new guest.
    #[must_use]
    pub fn encode_full_state(&self) -> Vec<u8> {
        self.doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default())
    }

    /// Drain the updates queued by the [`EditSink`] forwarding calls since the
    /// last drain — the once-per-frame broadcast seam (the `take_edits`
    /// idiom): the widget forwards edits as they happen, the session flushes
    /// the batch to the bus at frame end.
    pub fn take_updates(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.outbox)
    }

    /// Encode everything this doc changed since `before` — the incremental
    /// broadcast payload for exactly one local transaction.
    fn encode_since(&self, before: &StateVector) -> Vec<u8> {
        self.doc.transact().encode_state_as_update_v1(before)
    }
}

impl EditSink for CollabDoc {
    fn forward_insert(&mut self, char_idx: usize, text: &str) {
        if let Some(update) = self.insert(char_idx, text) {
            self.outbox.push(update);
        }
    }

    fn forward_remove(&mut self, range: Range<usize>) {
        if let Some(update) = self.remove(range) {
            self.outbox.push(update);
        }
    }
}

/// The yrs byte offset of char `char_idx` in `rope` (O(log n)).
///
/// # Panics
/// Panics if `char_idx` is past the end (`ropey`'s contract) or the offset
/// exceeds `u32::MAX` (yrs's index width; a >4 GiB text buffer is out of
/// scope for a code editor).
fn byte_index(rope: &Rope, char_idx: usize) -> u32 {
    u32::try_from(rope.char_to_byte(char_idx)).expect("text exceeds u32 byte range")
}

/// The byte offset of char `char_idx` in `s` (`s.len()` when past the last
/// char) — the &str sibling of `byte_index` for the diff below.
fn byte_of_char(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(b, _)| b)
}

/// The minimal single-span char edit script turning `old` into `new`: the
/// common prefix and suffix are pinned, the differing middle becomes at most
/// one [`TextEdit::Remove`] + one [`TextEdit::Insert`] (both at the prefix).
///
/// A merge that changed several disjoint regions collapses into one span —
/// coarser than op-exact, but any script that transforms `old` into `new` is
/// correct for the buffer bridge, and the common case (one remote keystroke
/// per update) stays exact. O(n).
fn char_span_diff(old: &str, new: &str) -> Vec<TextEdit> {
    let old_len = old.chars().count();
    let new_len = new.chars().count();
    let shared = old_len.min(new_len);

    let mut prefix = 0;
    for (a, b) in old.chars().zip(new.chars()) {
        if a != b {
            break;
        }
        prefix += 1;
    }
    // The prefix and suffix must never claim the same chars ("aa" → "aaa"
    // would otherwise count the middle "a" twice), so the suffix scan is
    // capped at what the prefix left unclaimed.
    let max_suffix = shared - prefix;
    let mut suffix = 0;
    for (a, b) in old.chars().rev().zip(new.chars().rev()) {
        if suffix == max_suffix || a != b {
            break;
        }
        suffix += 1;
    }

    let mut edits = Vec::new();
    if old_len - suffix > prefix {
        edits.push(TextEdit::Remove {
            range: prefix..old_len - suffix,
        });
    }
    let ins_start = byte_of_char(new, prefix);
    let ins_end = byte_of_char(new, new_len - suffix);
    if ins_end > ins_start {
        edits.push(TextEdit::Insert {
            at: prefix,
            text: new[ins_start..ins_end].to_string(),
        });
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::{char_span_diff, CollabDoc, CrdtError, EditSink, TextEdit};
    use crate::buffer::Buffer;

    /// A guest replica joining `host`'s session: starts empty (bridge contract
    /// rule 1 — never re-seed) and bootstraps from the host's full state.
    fn join(host: &CollabDoc, client_id: u64) -> CollabDoc {
        let mut guest = CollabDoc::new(client_id);
        guest
            .apply_remote(&host.encode_full_state())
            .expect("bootstrap join");
        guest
    }

    /// One full y-sync handshake in both directions — exactly the exchange
    /// COLLAB-2 carries over mde-bus: swap state vectors, answer with diffs,
    /// apply. After this, `a` and `b` have seen the same op set.
    fn sync(a: &mut CollabDoc, b: &mut CollabDoc) {
        let to_b = a.diff(&b.encode_state_vector()).expect("diff a→b");
        let to_a = b.diff(&a.encode_state_vector()).expect("diff b→a");
        a.apply_remote(&to_a).expect("apply b→a");
        b.apply_remote(&to_b).expect("apply a→b");
    }

    /// Replay a remote script onto a plain [`Buffer`] — the COLLAB-2 wiring's
    /// remote leg, verbatim.
    fn replay(buffer: &mut Buffer, edits: &[TextEdit]) {
        for edit in edits {
            edit.apply_to(buffer);
        }
    }

    // ---- convergence -------------------------------------------------------

    #[test]
    fn two_docs_exchanging_updates_converge_in_both_delivery_orders() {
        let mut a = CollabDoc::from_text("the quick fox\n", 1);
        let base = a.encode_full_state();
        let mut b = join(&a, 2);

        let pa = a.insert(0, "A: ").expect("a's payload");
        let end = b.len_chars();
        let pb = b.insert(end, "B: tail\n").expect("b's payload");

        a.apply_remote(&pb).expect("a merges b");
        b.apply_remote(&pa).expect("b merges a");
        assert_eq!(a.to_text(), b.to_text(), "peers must converge");
        assert_eq!(a.to_text(), "A: the quick fox\nB: tail\n");

        // Two observers receiving the SAME payloads in OPPOSITE orders land on
        // the identical text — delivery order must not matter.
        let mut c = CollabDoc::new(3);
        let mut d = CollabDoc::new(4);
        c.apply_remote(&base).expect("c base");
        d.apply_remote(&base).expect("d base");
        c.apply_remote(&pa).expect("c: a first");
        c.apply_remote(&pb).expect("c: b second");
        d.apply_remote(&pb).expect("d: b first");
        d.apply_remote(&pa).expect("d: a second");
        assert_eq!(c.to_text(), d.to_text(), "order-independent convergence");
        assert_eq!(c.to_text(), a.to_text());
    }

    #[test]
    fn causally_dependent_update_parks_until_its_predecessor_arrives() {
        let mut a = CollabDoc::from_text("base ", 1);
        let mut b = join(&a, 2);

        let u1 = a.insert(5, "one ").expect("u1");
        let u2 = a.insert(9, "two ").expect("u2 (depends on u1)");

        // Newest-first delivery: u2 alone cannot integrate (its causal
        // predecessor is missing), so it parks and the text is untouched —
        // offline tolerance without transport-level sequencing.
        let parked = b.apply_remote(&u2).expect("parking is not an error");
        assert!(parked.is_empty(), "a parked update edits nothing");
        assert_eq!(b.to_text(), "base ");

        // The predecessor arrives → both integrate.
        b.apply_remote(&u1).expect("u1 unlocks u2");
        assert_eq!(b.to_text(), "base one two ");
        assert_eq!(b.to_text(), a.to_text());
    }

    #[test]
    fn concurrent_inserts_at_the_same_position_converge_identically() {
        let mut a = CollabDoc::from_text("ab", 1);
        let base = a.encode_full_state();
        let mut b = join(&a, 2);

        // The classic conflict: both peers insert at char 1 concurrently.
        let pa = a.insert(1, "X").expect("a's payload");
        let pb = b.insert(1, "Y").expect("b's payload");
        a.apply_remote(&pb).expect("a merges b");
        b.apply_remote(&pa).expect("b merges a");

        assert_eq!(
            a.to_text(),
            b.to_text(),
            "conflict must resolve identically"
        );
        let merged = a.to_text();
        assert!(
            merged == "aXYb" || merged == "aYXb",
            "both inserts survive, deterministically ordered (got {merged:?})"
        );

        // And the resolution is delivery-order independent too.
        let mut c = CollabDoc::new(3);
        let mut d = CollabDoc::new(4);
        c.apply_remote(&base).expect("c base");
        d.apply_remote(&base).expect("d base");
        c.apply_remote(&pa).expect("c: a first");
        c.apply_remote(&pb).expect("c: b second");
        d.apply_remote(&pb).expect("d: b first");
        d.apply_remote(&pa).expect("d: a second");
        assert_eq!(c.to_text(), merged);
        assert_eq!(d.to_text(), merged);
    }

    #[test]
    fn three_way_merge_converges() {
        let mut a = CollabDoc::from_text("shared\n", 1);
        let mut b = join(&a, 2);
        let mut c = join(&a, 3);

        // Three peers edit concurrently — two of them at the same position.
        let _ = a.insert(0, "a| ").expect("a edit");
        let _ = b.insert(0, "b| ").expect("b edit");
        let end = c.len_chars();
        let _ = c.insert(end, "c-line\n").expect("c edit");

        // Gossip pairwise until quiescent (a relays b↔c).
        sync(&mut a, &mut b);
        sync(&mut a, &mut c);
        sync(&mut a, &mut b);

        assert_eq!(a.to_text(), b.to_text(), "a/b diverged");
        assert_eq!(b.to_text(), c.to_text(), "b/c diverged");
        let merged = a.to_text();
        for marker in ["a| ", "b| ", "shared\n", "c-line\n"] {
            assert!(merged.contains(marker), "{marker:?} lost in {merged:?}");
        }
    }

    #[test]
    fn offline_batches_reconcile_through_the_state_vector_handshake() {
        let mut a = CollabDoc::from_text("doc v1\n", 1);
        let mut b = join(&a, 2);

        // Both peers edit OFFLINE: every broadcast payload is dropped on the
        // floor (local-first — each doc stays immediately editable).
        let _ = a.insert(0, "a-one ").expect("a1");
        let end = a.len_chars();
        let _ = a.insert(end, "a-two\n").expect("a2");
        let _ = a.remove(0..2).expect("a3 trims 'a-'");
        let end = b.len_chars();
        let _ = b.insert(end, "b-one\n").expect("b1");
        let _ = b.insert(0, "b-two ").expect("b2");

        assert_ne!(a.to_text(), b.to_text(), "genuinely diverged while offline");

        // Reconnect = one handshake; no payload replay, no sequence numbers.
        sync(&mut a, &mut b);

        assert_eq!(a.to_text(), b.to_text(), "reconnect must reconcile");
        let merged = a.to_text();
        for marker in ["one ", "a-two\n", "b-one\n", "b-two ", "doc v1\n"] {
            assert!(merged.contains(marker), "{marker:?} lost in {merged:?}");
        }
    }

    // ---- the sync seam -----------------------------------------------------

    #[test]
    fn diff_sends_only_what_the_peer_is_missing() {
        let long_base = "fn main() {\n    println!(\"hello mesh\");\n}\n".repeat(8);
        let mut a = CollabDoc::from_text(&long_base, 1);
        let mut b = join(&a, 2);

        // Fully synced: the diff carries nothing (applies as an empty script).
        let idle = a.diff(&b.encode_state_vector()).expect("idle diff");
        assert!(
            b.apply_remote(&idle).expect("idle apply").is_empty(),
            "a synced peer must receive no edits"
        );

        // One tiny edit → the diff carries that edit, NOT the whole history.
        let _ = a.insert(0, "// x\n").expect("small edit");
        let delta = a.diff(&b.encode_state_vector()).expect("delta diff");
        let full = a.encode_full_state();
        assert!(
            delta.len() < full.len() / 4,
            "delta ({} bytes) must be far smaller than the full state ({} bytes)",
            delta.len(),
            full.len()
        );

        let edits = b.apply_remote(&delta).expect("delta apply");
        assert_eq!(
            edits,
            vec![TextEdit::Insert {
                at: 0,
                text: "// x\n".into()
            }],
            "the delta replays as exactly the missing edit"
        );
        assert_eq!(a.to_text(), b.to_text());
    }

    #[test]
    fn apply_remote_is_idempotent() {
        let mut a = CollabDoc::from_text("stable", 1);
        let mut b = join(&a, 2);

        let payload = a.insert(6, "!").expect("payload");
        let first = b.apply_remote(&payload).expect("first delivery");
        assert!(!first.is_empty(), "the first delivery edits");
        let again = b.apply_remote(&payload).expect("redelivery");
        assert!(again.is_empty(), "a redelivered update edits nothing");
        assert_eq!(b.to_text(), "stable!");
    }

    #[test]
    fn malformed_payloads_error_honestly() {
        let mut doc = CollabDoc::from_text("safe", 1);
        let garbage = [0xff_u8, 0x13, 0x37, 0xff, 0x00, 0x42];

        assert!(
            matches!(doc.apply_remote(&garbage), Err(CrdtError::Decode(_))),
            "garbage update bytes must be a Decode error"
        );
        assert!(
            matches!(doc.diff(&garbage), Err(CrdtError::Decode(_))),
            "garbage state-vector bytes must be a Decode error"
        );
        assert_eq!(doc.to_text(), "safe", "a failed apply changes nothing");
    }

    // ---- the Buffer bridge -------------------------------------------------

    #[test]
    fn from_buffer_round_trips_to_text() {
        let buffer = Buffer::from_text("línea one\n🌍 line two\nthree\n");
        let doc = CollabDoc::from_buffer(&buffer, 1);
        assert_eq!(doc.to_text(), buffer.rope().to_string());
        assert!(doc.in_sync_with(&buffer));
        assert_eq!(doc.client_id(), 1);
        assert_eq!(doc.len_chars(), buffer.len_chars());
    }

    #[test]
    fn edit_sink_forwarding_keeps_doc_buffer_and_peer_in_lockstep() {
        // The full bridge contract, end to end: the host mirrors every Buffer
        // mutation through the EditSink seam, drains the outbox once per
        // "frame", and a guest applying those payloads lands on the same text.
        let mut buffer = Buffer::from_text("fn main() {}\n");
        let mut host = CollabDoc::from_buffer(&buffer, 1);
        let mut guest = join(&host, 2);

        // Contract rule 2: twin calls, same args, same order.
        buffer.insert(3, "x");
        host.forward_insert(3, "x");
        buffer.remove(0..2);
        host.forward_remove(0..2);
        let end = buffer.len_chars();
        buffer.insert(end, "// done\n");
        host.forward_insert(end, "// done\n");

        assert!(host.in_sync_with(&buffer), "host must track the buffer");
        assert_eq!(host.to_text(), buffer.rope().to_string());

        // Frame end: drain the outbox and "broadcast".
        let updates = host.take_updates();
        assert_eq!(updates.len(), 3, "one payload per forwarded mutation");
        assert!(
            host.take_updates().is_empty(),
            "the drain resets the outbox"
        );
        for update in &updates {
            guest.apply_remote(update).expect("guest merges");
        }
        assert_eq!(guest.to_text(), buffer.rope().to_string());

        // No-op mutations forward as nothing at all — including a REVERSED
        // range, which mirrors `Buffer::remove`'s start >= end no-op contract
        // (the literal is deliberate; that is the very case under test).
        host.forward_insert(0, "");
        host.forward_remove(5..5);
        #[allow(clippy::reversed_empty_ranges)]
        host.forward_remove(4..1);
        assert!(host.take_updates().is_empty(), "no-ops queue no payloads");
        assert_eq!(host.insert(0, ""), None);
        assert_eq!(host.remove(2..2), None);
    }

    #[test]
    fn remote_scripts_replay_onto_a_buffer_across_unicode() {
        // Multi-byte chars on both sides of the edits prove the char↔byte
        // bridge: yrs speaks bytes, Buffer speaks chars, and the script that
        // crosses back must be char-correct.
        let base = "héllo 🌍 wörld\n";
        let mut a = CollabDoc::from_text(base, 1);
        let mut b = join(&a, 2);
        let mut buffer = Buffer::from_text(base); // b's editor-side buffer

        let p1 = a.insert(8, "✨ ").expect("insert after the emoji");
        let p2 = a.remove(0..2).expect("remove 'hé'");

        for payload in [&p1, &p2] {
            let script = b.apply_remote(payload).expect("merge");
            replay(&mut buffer, &script);
        }

        assert_eq!(b.to_text(), a.to_text());
        assert_eq!(buffer.rope().to_string(), b.to_text());
        assert!(b.in_sync_with(&buffer), "contract rule 3 held");
        assert_eq!(a.to_text(), "llo 🌍 ✨ wörld\n");
    }

    // ---- the diff primitive ------------------------------------------------

    #[test]
    fn char_span_diff_pins_prefix_and_suffix() {
        assert_eq!(char_span_diff("same", "same"), vec![]);
        assert_eq!(
            char_span_diff("ab", "aXb"),
            vec![TextEdit::Insert {
                at: 1,
                text: "X".into()
            }]
        );
        assert_eq!(
            char_span_diff("aXb", "ab"),
            vec![TextEdit::Remove { range: 1..2 }]
        );
        assert_eq!(
            char_span_diff("aXb", "aYb"),
            vec![
                TextEdit::Remove { range: 1..2 },
                TextEdit::Insert {
                    at: 1,
                    text: "Y".into()
                }
            ]
        );
        // The prefix must not double-claim chars the suffix wants.
        assert_eq!(
            char_span_diff("aa", "aaa"),
            vec![TextEdit::Insert {
                at: 2,
                text: "a".into()
            }]
        );
        // Char (not byte) positions around multi-byte scalars.
        assert_eq!(
            char_span_diff("🌍🌍", "🌍x🌍"),
            vec![TextEdit::Insert {
                at: 1,
                text: "x".into()
            }]
        );
    }
}
