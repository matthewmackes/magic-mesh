//! The transactional, idempotent, convergent **SQLite projection**.
//!
//! [`Projection::project`] folds a batch of signed events into the materialized
//! read tables that back the [`CollabReadModel`] shapes. It is:
//!
//! * **Idempotent** — every event first lands in the durable `collab_event`
//!   table via `INSERT OR IGNORE` (keyed by [`EventId`]); re-applying an event is
//!   a no-op.
//! * **Order-independent** — the materialized rows for a space are **rebuilt**
//!   on every batch by folding that space's *entire* event set from
//!   `collab_event` in the canonical order `(clock, event_id)`. The rebuild is a
//!   pure function of the event SET, so two nodes that have accepted the same
//!   events hold byte-identical tables regardless of arrival order.
//! * **Transactional** — the ingest + rebuild run in a single transaction.
//!
//! Read-side helpers reconstruct the typed [`CollabReadModel`] structs from the
//! tables with deterministic `ORDER BY`s. [`Projection::dump_tables`] serializes
//! every materialized table for the convergence assertion.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mde_collab_types::event::CollabEventKind;
use mde_collab_types::ids::{
    CallId, DocumentId, EventId, FileRefId, SpaceId, ThreadId, TransferId,
};
use mde_collab_types::read_model::{
    ActivityEntry, ActivityFeed, AlertInbox, AlertView, CallParticipantView, CallState, CallView,
    ClipboardLane, ClipboardView, ConversationTimeline, DocumentSession, DocumentSessions,
    FileReferenceView, FileReferences, MessageView, PresenceBoard, PresenceView, SpaceDirectory,
    SpaceSummary, ThreadTimeline, TransferJobView, TransferJobs,
};
use mde_collab_types::value::{
    AlertPayload, CallKind, CallParticipantState, ClipItemKind, DeliveryState, FileRef,
    PresenceState, TransferDirection, TransferMethod, TransferState,
};
use mde_collab_types::{ActorClock, ActorId, CollabEventEnvelope, SpaceKind, SpaceRole};
use rusqlite::{params, Connection, OptionalExtension};

use crate::domain::sort_key;
use crate::error::{CollabError, Result};

const SCHEMA: &str = include_str!("../migrations/0001_init.sql");

/// The materialized read tables, in a fixed order — used by [`dump_tables`] and
/// the per-space rebuild.
const SPACE_TABLES: &[&str] = &[
    "spaces",
    "members",
    "messages",
    "threads",
    "alerts",
    "clipboard",
    "file_refs",
    "transfers",
    "documents",
    "calls",
];

/// The transactional read-side projection over a SQLite connection.
pub struct Projection {
    conn: Connection,
}

impl Projection {
    /// Open an in-memory projection (tests, transient).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    /// Open (creating) an on-disk projection at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Borrow the underlying connection (read-only queries the caller composes).
    #[must_use]
    pub const fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Fold a batch of signed events into the projection. Idempotent + rebuilds
    /// every touched space (and the global presence board) from the canonical
    /// event order, all in one transaction.
    pub fn project(&mut self, events: &[CollabEventEnvelope]) -> Result<()> {
        let tx = self.conn.transaction()?;
        let mut touched: BTreeSet<SpaceId> = BTreeSet::new();
        let mut presence_touched = false;
        for env in events {
            let json = serde_json::to_string(env)?;
            let changed = tx.execute(
                "INSERT OR IGNORE INTO collab_event \
                 (event_id, space_id, actor, clock_wall, clock_counter, created_ms, kind_tag, envelope_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    env.event_id.to_string(),
                    env.space_id.to_string(),
                    env.actor.as_str(),
                    i64::try_from(env.clock.wall_ms).unwrap_or(i64::MAX),
                    i64::from(env.clock.counter),
                    env.created_unix_ms,
                    env.kind.tag(),
                    json,
                ],
            )?;
            let _ = changed;
            if !env.space_id.is_nil() {
                touched.insert(env.space_id);
            }
            if matches!(env.kind, CollabEventKind::PresenceChanged { .. }) {
                presence_touched = true;
            }
        }
        for space in &touched {
            rebuild_space(&tx, *space)?;
        }
        if presence_touched {
            rebuild_presence(&tx)?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---- Read-side reconstruction --------------------------------------

    /// The rail directory of spaces `viewer` is a present member of.
    pub fn space_directory(&self, viewer: &ActorId) -> Result<SpaceDirectory> {
        let mut stmt = self.conn.prepare(
            "SELECT s.space_id, s.kind, s.name, s.last_clock_wall, s.last_clock_counter, m.role, \
             (SELECT COUNT(*) FROM members m2 WHERE m2.space_id = s.space_id) \
             FROM spaces s JOIN members m ON m.space_id = s.space_id \
             WHERE s.deleted = 0 AND m.actor = ?1 \
             ORDER BY s.last_clock_wall DESC, s.last_clock_counter DESC, s.space_id",
        )?;
        let rows = stmt.query_map(params![viewer.as_str()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?;
        let mut spaces = Vec::new();
        for row in rows {
            let (id, kind, name, cw, cc, role, members) = row?;
            spaces.push(SpaceSummary {
                id: parse_space(&id)?,
                kind: parse_json_enum(&kind)?,
                name,
                role: parse_json_enum(&role)?,
                // Unread is a seat-local read-cursor rollup — WL-FUNC-011 Phase 1
                // follow-up: per-seat read markers are not in the replicated log.
                unread: 0,
                members: u32::try_from(members).unwrap_or(0),
                last_activity: clock(cw, cc),
            });
        }
        Ok(SpaceDirectory { spaces })
    }

    /// A conversation timeline (main timeline when `thread` is `None`).
    pub fn conversation_timeline(
        &self,
        space: SpaceId,
        thread: Option<ThreadId>,
    ) -> Result<ConversationTimeline> {
        let messages = self.messages_where(space, thread)?;
        Ok(ConversationTimeline {
            space,
            thread,
            messages,
        })
    }

    /// A thread's root message + ordered replies + resolved flag.
    pub fn thread_timeline(&self, space: SpaceId, thread: ThreadId) -> Result<ThreadTimeline> {
        let (root_event_id, resolved): (String, i64) = self.conn.query_row(
            "SELECT root_event_id, resolved FROM threads WHERE thread_id = ?1",
            params![thread.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let root_id = parse_event(&root_event_id)?;
        let root = self
            .message_by_id(root_id)?
            .ok_or(CollabError::MessageNotFound(root_id))?;
        let replies = self.messages_where(space, Some(thread))?;
        Ok(ThreadTimeline {
            space,
            thread,
            root,
            replies,
            resolved: resolved != 0,
        })
    }

    /// The global alert inbox, newest-first.
    pub fn alert_inbox(&self) -> Result<AlertInbox> {
        let mut stmt = self.conn.prepare(
            "SELECT space_id, event_id, severity, source, headline, fields_json, actions_json, \
             goto, acknowledged, snoozed_until \
             FROM alerts ORDER BY clock_wall DESC, clock_counter DESC, event_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, Option<i64>>(9)?,
            ))
        })?;
        let mut alerts = Vec::new();
        for row in rows {
            let (
                space,
                event,
                severity,
                source,
                headline,
                fields_json,
                actions_json,
                goto,
                ack,
                snz,
            ) = row?;
            let payload = AlertPayload {
                severity: parse_json_enum(&severity)?,
                source,
                headline,
                fields: serde_json::from_str(&fields_json)?,
                actions: serde_json::from_str(&actions_json)?,
                goto,
            };
            alerts.push(AlertView {
                event_id: parse_event(&event)?,
                space: parse_space(&space)?,
                alert: payload,
                acknowledged: ack != 0,
                snoozed_until_unix_ms: snz,
            });
        }
        Ok(AlertInbox { alerts })
    }

    /// A space's clipboard lane, newest-first.
    pub fn clipboard_lane(&self, space: SpaceId) -> Result<ClipboardLane> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, kind, preview, sha256_hex, source, at_ms, pinned \
             FROM clipboard WHERE space_id = ?1 \
             ORDER BY clock_wall DESC, clock_counter DESC, event_id",
        )?;
        let rows = stmt.query_map(params![space.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?;
        let mut items = Vec::new();
        for row in rows {
            let (event, kind, preview, sha, source, at_ms, pinned) = row?;
            items.push(ClipboardView {
                event_id: parse_event(&event)?,
                kind: parse_json_enum::<ClipItemKind>(&kind)?,
                preview,
                sha256_hex: sha,
                source,
                at_unix_ms: at_ms,
                pinned: pinned != 0,
            });
        }
        Ok(ClipboardLane { space, items })
    }

    /// The global presence board.
    pub fn presence_board(&self) -> Result<PresenceBoard> {
        let mut stmt = self
            .conn
            .prepare("SELECT actor, presence, status FROM presence ORDER BY actor")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        let mut members = Vec::new();
        for row in rows {
            let (actor, presence, status) = row?;
            members.push(PresenceView {
                actor: ActorId::new(actor),
                presence: parse_json_enum::<PresenceState>(&presence)?,
                status,
                role_badge: None,
            });
        }
        Ok(PresenceBoard { members })
    }

    /// A space's linked file references.
    pub fn file_references(&self, space: SpaceId) -> Result<FileReferences> {
        let mut stmt = self.conn.prepare(
            "SELECT file_ref_id, name, size, sha256_hex, mime, linked_by, linked_ms \
             FROM file_refs WHERE space_id = ?1 \
             ORDER BY clock_wall, clock_counter, file_ref_id",
        )?;
        let rows = stmt.query_map(params![space.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?;
        let mut files = Vec::new();
        for row in rows {
            let (id, name, size, sha, mime, by, at) = row?;
            files.push(FileReferenceView {
                file: parse_fileref(&id)?,
                reference: FileRef {
                    name,
                    size: u64::try_from(size).unwrap_or(0),
                    sha256_hex: sha,
                    mime,
                },
                linked_by: ActorId::new(by),
                linked_unix_ms: at,
            });
        }
        Ok(FileReferences { space, files })
    }

    /// All transfer jobs (the read-side mirror).
    pub fn transfer_jobs(&self) -> Result<TransferJobs> {
        let mut stmt = self.conn.prepare(
            "SELECT transfer_id, file_ref_id, method, direction, state, moved, total \
             FROM transfers ORDER BY clock_wall, clock_counter, transfer_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?;
        let mut jobs = Vec::new();
        for row in rows {
            let (id, file, method, direction, state, moved, total) = row?;
            jobs.push(TransferJobView {
                transfer: parse_transfer(&id)?,
                file: parse_fileref(&file)?,
                method: parse_json_enum::<TransferMethod>(&method)?,
                direction: parse_json_enum::<TransferDirection>(&direction)?,
                state: parse_json_enum::<TransferState>(&state)?,
                moved: u64::try_from(moved).unwrap_or(0),
                total: u64::try_from(total).unwrap_or(0),
            });
        }
        Ok(TransferJobs { jobs })
    }

    /// The active/ended calls (optionally scoped to one space).
    pub fn call_state(&self, space: Option<SpaceId>) -> Result<CallState> {
        let (sql, want) = match space {
            Some(s) => (
                "SELECT call_id, space_id, kind, initiator, started_ms, ended FROM calls \
                 WHERE space_id = ?1 ORDER BY clock_wall, clock_counter, call_id"
                    .to_string(),
                Some(s.to_string()),
            ),
            None => (
                "SELECT call_id, space_id, kind, initiator, started_ms, ended FROM calls \
                 ORDER BY clock_wall, clock_counter, call_id"
                    .to_string(),
                None,
            ),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let map = |r: &rusqlite::Row<'_>| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        };
        let collected: Vec<_> = if let Some(w) = want {
            stmt.query_map(params![w], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut active = Vec::new();
        for (call, space_id, kind, _initiator, started, ended) in collected {
            if ended != 0 {
                continue;
            }
            let call_id = parse_call(&call)?;
            active.push(CallView {
                call: call_id,
                space: parse_space(&space_id)?,
                kind: parse_json_enum::<CallKind>(&kind)?,
                started_unix_ms: started,
                participants: self.call_participants(call_id)?,
            });
        }
        Ok(CallState { active })
    }

    /// The live document sessions (optionally scoped to one space).
    pub fn document_sessions(&self, space: Option<SpaceId>) -> Result<DocumentSessions> {
        let base = "SELECT space_id, document_id, title, participants_json FROM documents";
        let (sql, want) = match space {
            Some(s) => (
                format!(
                    "{base} WHERE space_id = ?1 ORDER BY clock_wall, clock_counter, document_id"
                ),
                Some(s.to_string()),
            ),
            None => (
                format!("{base} ORDER BY clock_wall, clock_counter, document_id"),
                None,
            ),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let map = |r: &rusqlite::Row<'_>| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        };
        let collected: Vec<_> = if let Some(w) = want {
            stmt.query_map(params![w], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut sessions = Vec::new();
        for (space_id, document, title, participants_json) in collected {
            let names: Vec<String> = serde_json::from_str(&participants_json)?;
            sessions.push(DocumentSession {
                document: parse_document(&document)?,
                space: parse_space(&space_id)?,
                title,
                participants: names.into_iter().map(ActorId::new).collect(),
                call: None,
            });
        }
        Ok(DocumentSessions { sessions })
    }

    /// A space's chronological Activity feed (newest-last).
    pub fn activity_feed(&self, space: SpaceId) -> Result<ActivityFeed> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, actor, clock_wall, clock_counter, created_ms, kind_tag \
             FROM collab_event WHERE space_id = ?1 \
             ORDER BY clock_wall, clock_counter, event_id",
        )?;
        let rows = stmt.query_map(params![space.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        let mut entries = Vec::new();
        for row in rows {
            let (event, actor, cw, cc, created, tag) = row?;
            entries.push(ActivityEntry {
                event_id: parse_event(&event)?,
                space,
                actor: ActorId::new(actor),
                clock: clock(cw, cc),
                created_unix_ms: created,
                summary: summarize(&tag),
                kind_tag: tag,
            });
        }
        Ok(ActivityFeed {
            space: Some(space),
            entries,
        })
    }

    fn call_participants(&self, call: CallId) -> Result<Vec<CallParticipantView>> {
        let mut stmt = self.conn.prepare(
            "SELECT actor, state, muted FROM call_participants WHERE call_id = ?1 ORDER BY actor",
        )?;
        let rows = stmt.query_map(params![call.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (actor, state, muted) = row?;
            out.push(CallParticipantView {
                actor: ActorId::new(actor),
                state: parse_json_enum::<CallParticipantState>(&state)?,
                muted: muted != 0,
            });
        }
        Ok(out)
    }

    fn messages_where(&self, space: SpaceId, thread: Option<ThreadId>) -> Result<Vec<MessageView>> {
        let sql = match thread {
            Some(_) => {
                "SELECT event_id, author, created_ms, body, edited, deleted \
                 FROM messages WHERE space_id = ?1 AND thread_id = ?2 \
                 ORDER BY clock_wall, clock_counter, event_id"
            }
            None => {
                "SELECT event_id, author, created_ms, body, edited, deleted \
                 FROM messages WHERE space_id = ?1 AND thread_id IS NULL \
                 ORDER BY clock_wall, clock_counter, event_id"
            }
        };
        let mut stmt = self.conn.prepare(sql)?;
        let map = |r: &rusqlite::Row<'_>| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        };
        let collected: Vec<_> = match thread {
            Some(t) => stmt
                .query_map(params![space.to_string(), t.to_string()], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?,
            None => stmt
                .query_map(params![space.to_string()], map)?
                .collect::<std::result::Result<Vec<_>, _>>()?,
        };
        let mut out = Vec::new();
        for (event, author, created, body, edited, deleted) in collected {
            let id = parse_event(&event)?;
            out.push(MessageView {
                event_id: id,
                author: ActorId::new(author),
                created_unix_ms: created,
                body,
                edited: edited != 0,
                deleted: deleted != 0,
                // Delivery derives from live recipient presence — WL-FUNC-011
                // Phase 1 follow-up: cross-reference the presence board at read
                // time. The honest default is Sent (published; reachability
                // unknown), never a fabricated read receipt.
                delivery: DeliveryState::Sent,
                reply_count: self.reply_count(id)?,
            });
        }
        Ok(out)
    }

    fn reply_count(&self, root: EventId) -> Result<u32> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE thread_id IN \
             (SELECT thread_id FROM threads WHERE root_event_id = ?1)",
            params![root.to_string()],
            |r| r.get(0),
        )?;
        Ok(u32::try_from(n).unwrap_or(0))
    }

    fn message_by_id(&self, id: EventId) -> Result<Option<MessageView>> {
        let row = self
            .conn
            .query_row(
                "SELECT author, created_ms, body, edited, deleted FROM messages WHERE event_id = ?1",
                params![id.to_string()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((author, created, body, edited, deleted)) = row else {
            return Ok(None);
        };
        Ok(Some(MessageView {
            event_id: id,
            author: ActorId::new(author),
            created_unix_ms: created,
            body,
            edited: edited != 0,
            deleted: deleted != 0,
            delivery: DeliveryState::Sent,
            reply_count: self.reply_count(id)?,
        }))
    }

    /// Serialize every materialized table (rows sorted by primary key) into one
    /// canonical string — the byte-for-byte convergence fingerprint. Two nodes
    /// that accepted the same event set produce the identical dump.
    pub fn dump_tables(&self) -> Result<String> {
        let mut out = String::new();
        for table in SPACE_TABLES {
            self.dump_one(table, &order_for(table), &mut out)?;
        }
        self.dump_one("call_participants", "call_id, actor", &mut out)?;
        self.dump_one("presence", "actor", &mut out)?;
        Ok(out)
    }

    fn dump_one(&self, table: &str, order_by: &str, out: &mut String) -> Result<()> {
        out.push_str("== ");
        out.push_str(table);
        out.push('\n');
        let sql = format!("SELECT * FROM {table} ORDER BY {order_by}");
        let mut stmt = self.conn.prepare(&sql)?;
        let cols: Vec<String> = stmt
            .column_names()
            .into_iter()
            .map(str::to_string)
            .collect();
        let count = cols.len();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            for (i, col) in cols.iter().enumerate().take(count) {
                let value: rusqlite::types::Value = row.get(i)?;
                out.push_str(col);
                out.push('=');
                out.push_str(&render_value(&value));
                out.push(';');
            }
            out.push('\n');
        }
        Ok(())
    }
}

// ---- The per-space + presence fold (the convergent rebuild) ---------------

fn rebuild_space(tx: &rusqlite::Transaction<'_>, space: SpaceId) -> Result<()> {
    // Clear this space's materialized rows (call_participants via its calls).
    let sid = space.to_string();
    tx.execute(
        "DELETE FROM call_participants WHERE call_id IN (SELECT call_id FROM calls WHERE space_id = ?1)",
        params![sid],
    )?;
    for table in SPACE_TABLES {
        tx.execute(
            &format!("DELETE FROM {table} WHERE space_id = ?1"),
            params![sid],
        )?;
    }

    // Load the space's full log in canonical order.
    let mut events = load_space_events(tx, space)?;
    events.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));

    let mut fold = SpaceFold::default();
    for env in &events {
        fold.apply(space, env);
    }
    fold.write(tx, space)?;
    Ok(())
}

fn rebuild_presence(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute("DELETE FROM presence", [])?;
    let mut stmt = tx.prepare(
        "SELECT envelope_json FROM collab_event WHERE kind_tag = 'presence_changed' \
         ORDER BY clock_wall, clock_counter, event_id",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    // LWW by ascending order: the last write for an actor wins.
    let mut latest: BTreeMap<String, (PresenceState, Option<String>, ActorClock)> = BTreeMap::new();
    for row in rows {
        let env: CollabEventEnvelope = serde_json::from_str(&row?)?;
        if let CollabEventKind::PresenceChanged {
            actor,
            presence,
            status,
        } = &env.kind
        {
            latest.insert(actor.0.clone(), (*presence, status.clone(), env.clock));
        }
    }
    for (actor, (presence, status, clk)) in latest {
        tx.execute(
            "INSERT INTO presence (actor, presence, status, clock_wall, clock_counter) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                actor,
                json_enum(&presence)?,
                status,
                i64::try_from(clk.wall_ms).unwrap_or(i64::MAX),
                i64::from(clk.counter),
            ],
        )?;
    }
    Ok(())
}

fn load_space_events(
    tx: &rusqlite::Transaction<'_>,
    space: SpaceId,
) -> Result<Vec<CollabEventEnvelope>> {
    let mut stmt = tx.prepare(
        "SELECT envelope_json FROM collab_event WHERE space_id = ?1 \
         ORDER BY clock_wall, clock_counter, event_id",
    )?;
    let rows = stmt.query_map(params![space.to_string()], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(serde_json::from_str(&row?)?);
    }
    Ok(out)
}

/// The in-memory resolution of one space's log into rows, applied in canonical
/// order so every derived field (LWW body/name/role/state, monotonic tombstones)
/// is order-independent.
#[derive(Default)]
struct SpaceFold {
    space: Option<SpaceRow>,
    members: BTreeMap<String, (SpaceRole, bool)>, // actor -> (role, present)
    messages: BTreeMap<EventId, MessageRow>,
    threads: BTreeMap<ThreadId, ThreadRow>,
    alerts: BTreeMap<EventId, AlertRow>,
    clipboard: BTreeMap<EventId, ClipRow>,
    files: BTreeMap<FileRefId, FileRow>,
    transfers: BTreeMap<TransferId, TransferRow>,
    documents: BTreeMap<DocumentId, DocRow>,
    calls: BTreeMap<CallId, CallRow>,
    last_clock: ActorClock,
}

struct SpaceRow {
    kind: SpaceKind,
    name: String,
    created_ms: i64,
    deleted: bool,
}
struct MessageRow {
    author: ActorId,
    created_ms: i64,
    thread: Option<ThreadId>,
    body: String,
    edited: bool,
    deleted: bool,
    clock: ActorClock,
}
struct ThreadRow {
    root: EventId,
    title: Option<String>,
    resolved: bool,
    clock: ActorClock,
}
struct AlertRow {
    payload: AlertPayload,
    acknowledged: bool,
    snoozed_until: Option<i64>,
    clock: ActorClock,
}
struct ClipRow {
    kind: ClipItemKind,
    preview: String,
    sha256_hex: String,
    source: String,
    at_ms: i64,
    pinned: bool,
    deleted: bool,
    clock: ActorClock,
}
struct FileRow {
    reference: FileRef,
    linked_by: ActorId,
    linked_ms: i64,
    present: bool,
    clock: ActorClock,
}
struct TransferRow {
    file: FileRefId,
    method: TransferMethod,
    direction: TransferDirection,
    state: TransferState,
    clock: ActorClock,
}
struct DocRow {
    title: String,
    latest_summary: Option<String>,
    participants: BTreeSet<String>,
    clock: ActorClock,
}
struct CallRow {
    kind: CallKind,
    initiator: ActorId,
    started_ms: i64,
    ended: bool,
    participants: BTreeMap<String, (CallParticipantState, bool)>,
    clock: ActorClock,
}

impl SpaceFold {
    #[allow(clippy::too_many_lines)]
    fn apply(&mut self, space: SpaceId, env: &CollabEventEnvelope) {
        let _ = space;
        if env.clock > self.last_clock {
            self.last_clock = env.clock;
        }
        match &env.kind {
            CollabEventKind::SpaceCreated { kind, name } => {
                self.space = Some(SpaceRow {
                    kind: *kind,
                    name: name.clone(),
                    created_ms: env.created_unix_ms,
                    deleted: false,
                });
            }
            CollabEventKind::SpaceRenamed { name } => {
                if let Some(s) = self.space.as_mut() {
                    s.name = name.clone();
                }
            }
            CollabEventKind::SpaceDeleted => {
                if let Some(s) = self.space.as_mut() {
                    s.deleted = true;
                }
            }
            CollabEventKind::SpaceArchived => {}
            CollabEventKind::MemberJoined { actor, role } => {
                self.members.insert(actor.0.clone(), (*role, true));
            }
            CollabEventKind::MemberLeft { actor } => {
                if let Some(m) = self.members.get_mut(&actor.0) {
                    m.1 = false;
                }
            }
            CollabEventKind::MemberRoleChanged { actor, role } => {
                if let Some(m) = self.members.get_mut(&actor.0) {
                    m.0 = *role;
                }
            }
            CollabEventKind::PresenceChanged { .. } => {}
            CollabEventKind::MessagePosted { body, thread } => {
                self.messages.insert(
                    env.event_id,
                    MessageRow {
                        author: env.actor.clone(),
                        created_ms: env.created_unix_ms,
                        thread: *thread,
                        body: body.0.clone(),
                        edited: false,
                        deleted: false,
                        clock: env.clock,
                    },
                );
            }
            CollabEventKind::MessageEdited { target, body } => {
                if let Some(m) = self.messages.get_mut(target) {
                    // Only the author's edits count; ascending order → last wins.
                    if m.author == env.actor && !m.deleted {
                        m.body = body.0.clone();
                        m.edited = true;
                    }
                }
            }
            CollabEventKind::MessageDeleted { target } => {
                if let Some(m) = self.messages.get_mut(target) {
                    if m.author == env.actor {
                        m.deleted = true;
                        m.body = String::new();
                    }
                }
            }
            CollabEventKind::ThreadStarted {
                thread,
                root,
                title,
            } => {
                self.threads.insert(
                    *thread,
                    ThreadRow {
                        root: *root,
                        title: title.clone(),
                        resolved: false,
                        clock: env.clock,
                    },
                );
            }
            CollabEventKind::ThreadResolved { thread } => {
                if let Some(t) = self.threads.get_mut(thread) {
                    t.resolved = true;
                }
            }
            CollabEventKind::AlertRaised { alert } => {
                self.alerts.insert(
                    env.event_id,
                    AlertRow {
                        payload: alert.clone(),
                        acknowledged: false,
                        snoozed_until: None,
                        clock: env.clock,
                    },
                );
            }
            CollabEventKind::AlertAcknowledged { target } => {
                if let Some(a) = self.alerts.get_mut(target) {
                    a.acknowledged = true;
                }
            }
            CollabEventKind::AlertSnoozed {
                target,
                until_unix_ms,
            } => {
                if let Some(a) = self.alerts.get_mut(target) {
                    a.snoozed_until = Some(*until_unix_ms);
                }
            }
            CollabEventKind::AlertActionInvoked { .. } => {}
            CollabEventKind::ClipboardPublished { item } => {
                self.clipboard.insert(
                    env.event_id,
                    ClipRow {
                        kind: item.kind,
                        preview: item.preview.clone(),
                        sha256_hex: item.sha256_hex.clone(),
                        source: item.source.clone(),
                        at_ms: env.created_unix_ms,
                        pinned: false,
                        deleted: false,
                        clock: env.clock,
                    },
                );
            }
            CollabEventKind::ClipboardPinned { target } => {
                if let Some(c) = self.clipboard.get_mut(target) {
                    c.pinned = true;
                }
            }
            CollabEventKind::ClipboardUnpinned { target } => {
                if let Some(c) = self.clipboard.get_mut(target) {
                    c.pinned = false;
                }
            }
            CollabEventKind::ClipboardDeleted { target } => {
                if let Some(c) = self.clipboard.get_mut(target) {
                    c.deleted = true;
                }
            }
            CollabEventKind::DocumentCreated { document, title } => {
                let mut participants = BTreeSet::new();
                participants.insert(env.actor.0.clone());
                self.documents.insert(
                    *document,
                    DocRow {
                        title: title.clone(),
                        latest_summary: None,
                        participants,
                        clock: env.clock,
                    },
                );
            }
            CollabEventKind::DocumentUpdated { document, change } => {
                if let Some(d) = self.documents.get_mut(document) {
                    if change.summary.is_some() {
                        d.latest_summary = change.summary.clone();
                    }
                    d.participants.insert(env.actor.0.clone());
                }
            }
            CollabEventKind::ReviewRequested {
                document,
                reviewers,
            } => {
                if let Some(d) = self.documents.get_mut(document) {
                    for r in reviewers {
                        d.participants.insert(r.0.clone());
                    }
                }
            }
            CollabEventKind::ReviewSubmitted { document, .. } => {
                if let Some(d) = self.documents.get_mut(document) {
                    d.participants.insert(env.actor.0.clone());
                }
            }
            CollabEventKind::FileLinked { file, reference } => {
                self.files.insert(
                    *file,
                    FileRow {
                        reference: reference.clone(),
                        linked_by: env.actor.clone(),
                        linked_ms: env.created_unix_ms,
                        present: true,
                        clock: env.clock,
                    },
                );
            }
            CollabEventKind::FileUnlinked { file } => {
                if let Some(f) = self.files.get_mut(file) {
                    f.present = false;
                }
            }
            CollabEventKind::TransferStarted {
                transfer,
                file,
                method,
                direction,
            } => {
                self.transfers.insert(
                    *transfer,
                    TransferRow {
                        file: *file,
                        method: *method,
                        direction: *direction,
                        state: TransferState::Queued,
                        clock: env.clock,
                    },
                );
            }
            CollabEventKind::TransferStateChanged { transfer, state } => {
                if let Some(t) = self.transfers.get_mut(transfer) {
                    t.state = *state;
                }
            }
            CollabEventKind::CallStarted {
                call,
                kind,
                initiator,
            } => {
                let mut participants = BTreeMap::new();
                participants.insert(
                    initiator.0.clone(),
                    (CallParticipantState::Connected, false),
                );
                self.calls.insert(
                    *call,
                    CallRow {
                        kind: *kind,
                        initiator: initiator.clone(),
                        started_ms: env.created_unix_ms,
                        ended: false,
                        participants,
                        clock: env.clock,
                    },
                );
            }
            CollabEventKind::CallParticipantChanged { call, actor, state } => {
                if let Some(c) = self.calls.get_mut(call) {
                    let muted = c.participants.get(&actor.0).is_some_and(|p| p.1);
                    c.participants.insert(actor.0.clone(), (*state, muted));
                }
            }
            CollabEventKind::CallEnded { call, .. } => {
                if let Some(c) = self.calls.get_mut(call) {
                    c.ended = true;
                }
            }
            CollabEventKind::AiSuggestionOffered { .. }
            | CollabEventKind::AiSuggestionResolved { .. } => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    fn write(&self, tx: &rusqlite::Transaction<'_>, space: SpaceId) -> Result<()> {
        let sid = space.to_string();
        let Some(sp) = &self.space else {
            // No SpaceCreated seen for this id (an orphan/out-of-order fragment);
            // nothing to materialize.
            return Ok(());
        };
        tx.execute(
            "INSERT INTO spaces (space_id, kind, name, created_ms, deleted, last_clock_wall, last_clock_counter) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                sid,
                json_enum(&sp.kind)?,
                sp.name,
                sp.created_ms,
                i64::from(sp.deleted),
                cw(self.last_clock),
                cc(self.last_clock),
            ],
        )?;
        for (actor, (role, present)) in &self.members {
            if *present {
                tx.execute(
                    "INSERT INTO members (space_id, actor, role) VALUES (?1, ?2, ?3)",
                    params![sid, actor, json_enum(role)?],
                )?;
            }
        }
        for (id, m) in &self.messages {
            tx.execute(
                "INSERT INTO messages (space_id, event_id, author, created_ms, thread_id, body, edited, deleted, delivery, clock_wall, clock_counter) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'sent', ?9, ?10)",
                params![
                    sid,
                    id.to_string(),
                    m.author.0,
                    m.created_ms,
                    m.thread.map(|t| t.to_string()),
                    m.body,
                    i64::from(m.edited),
                    i64::from(m.deleted),
                    cw(m.clock),
                    cc(m.clock),
                ],
            )?;
        }
        for (id, t) in &self.threads {
            tx.execute(
                "INSERT INTO threads (space_id, thread_id, root_event_id, title, resolved, clock_wall, clock_counter) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    sid,
                    id.to_string(),
                    t.root.to_string(),
                    t.title,
                    i64::from(t.resolved),
                    cw(t.clock),
                    cc(t.clock),
                ],
            )?;
        }
        for (id, a) in &self.alerts {
            tx.execute(
                "INSERT INTO alerts (space_id, event_id, severity, source, headline, fields_json, actions_json, goto, acknowledged, snoozed_until, clock_wall, clock_counter) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    sid,
                    id.to_string(),
                    json_enum(&a.payload.severity)?,
                    a.payload.source,
                    a.payload.headline,
                    serde_json::to_string(&a.payload.fields)?,
                    serde_json::to_string(&a.payload.actions)?,
                    a.payload.goto,
                    i64::from(a.acknowledged),
                    a.snoozed_until,
                    cw(a.clock),
                    cc(a.clock),
                ],
            )?;
        }
        for (id, c) in &self.clipboard {
            if c.deleted {
                continue;
            }
            tx.execute(
                "INSERT INTO clipboard (space_id, event_id, kind, preview, sha256_hex, source, at_ms, pinned, clock_wall, clock_counter) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    sid,
                    id.to_string(),
                    json_enum(&c.kind)?,
                    c.preview,
                    c.sha256_hex,
                    c.source,
                    c.at_ms,
                    i64::from(c.pinned),
                    cw(c.clock),
                    cc(c.clock),
                ],
            )?;
        }
        for (id, f) in &self.files {
            if !f.present {
                continue;
            }
            tx.execute(
                "INSERT INTO file_refs (space_id, file_ref_id, name, size, sha256_hex, mime, linked_by, linked_ms, clock_wall, clock_counter) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    sid,
                    id.to_string(),
                    f.reference.name,
                    i64::try_from(f.reference.size).unwrap_or(i64::MAX),
                    f.reference.sha256_hex,
                    f.reference.mime,
                    f.linked_by.0,
                    f.linked_ms,
                    cw(f.clock),
                    cc(f.clock),
                ],
            )?;
        }
        for (id, t) in &self.transfers {
            tx.execute(
                "INSERT INTO transfers (transfer_id, space_id, file_ref_id, method, direction, state, moved, total, clock_wall, clock_counter) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, ?7, ?8)",
                params![
                    id.to_string(),
                    sid,
                    t.file.to_string(),
                    json_enum(&t.method)?,
                    json_enum(&t.direction)?,
                    json_enum(&t.state)?,
                    cw(t.clock),
                    cc(t.clock),
                ],
            )?;
        }
        for (id, d) in &self.documents {
            let participants: Vec<&String> = d.participants.iter().collect();
            tx.execute(
                "INSERT INTO documents (space_id, document_id, title, latest_summary, participants_json, clock_wall, clock_counter) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    sid,
                    id.to_string(),
                    d.title,
                    d.latest_summary,
                    serde_json::to_string(&participants)?,
                    cw(d.clock),
                    cc(d.clock),
                ],
            )?;
        }
        for (id, c) in &self.calls {
            tx.execute(
                "INSERT INTO calls (call_id, space_id, kind, initiator, started_ms, ended, clock_wall, clock_counter) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    id.to_string(),
                    sid,
                    json_enum(&c.kind)?,
                    c.initiator.0,
                    c.started_ms,
                    i64::from(c.ended),
                    cw(c.clock),
                    cc(c.clock),
                ],
            )?;
            for (actor, (state, muted)) in &c.participants {
                tx.execute(
                    "INSERT INTO call_participants (call_id, actor, state, muted) VALUES (?1, ?2, ?3, ?4)",
                    params![id.to_string(), actor, json_enum(state)?, i64::from(*muted)],
                )?;
            }
        }
        Ok(())
    }
}

// ---- small helpers --------------------------------------------------------

const fn cw(c: ActorClock) -> i64 {
    // wall_ms fits i64 for any realistic epoch-ms; saturate defensively.
    if c.wall_ms > i64::MAX as u64 {
        i64::MAX
    } else {
        c.wall_ms as i64
    }
}

const fn cc(c: ActorClock) -> i64 {
    c.counter as i64
}

fn clock(wall: i64, counter: i64) -> ActorClock {
    ActorClock::at(
        u64::try_from(wall).unwrap_or(0),
        u32::try_from(counter).unwrap_or(0),
    )
}

fn order_for(table: &str) -> String {
    match table {
        "spaces" => "space_id".into(),
        "members" => "space_id, actor".into(),
        "messages" | "alerts" | "clipboard" => "event_id".into(),
        "threads" => "thread_id".into(),
        "file_refs" => "file_ref_id".into(),
        "transfers" => "transfer_id".into(),
        "documents" => "document_id".into(),
        "calls" => "call_id".into(),
        other => other.to_string(),
    }
}

fn render_value(v: &rusqlite::types::Value) -> String {
    use rusqlite::types::Value;
    match v {
        Value::Null => "NULL".to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => format!("{f}"),
        Value::Text(s) => s.clone(),
        Value::Blob(b) => format!("blob:{}", b.len()),
    }
}

/// The lower-`snake_case` wire tag of a serde enum (the value the read-model
/// serializes to), via the contracts' own serialization. Used to store an enum
/// as its stable string in a column.
fn json_enum<T: serde::Serialize>(value: &T) -> Result<String> {
    let s = serde_json::to_string(value)?;
    Ok(s.trim_matches('"').to_string())
}

/// The inverse of [`json_enum`]: parse a stored wire tag back into the enum.
fn parse_json_enum<T: serde::de::DeserializeOwned>(tag: &str) -> Result<T> {
    Ok(serde_json::from_str(&format!("\"{tag}\""))?)
}

fn parse_space(s: &str) -> Result<SpaceId> {
    s.parse()
        .map_err(|_| CollabError::Serde(format!("bad space id {s}")))
}
fn parse_event(s: &str) -> Result<EventId> {
    s.parse()
        .map_err(|_| CollabError::Serde(format!("bad event id {s}")))
}
fn parse_document(s: &str) -> Result<DocumentId> {
    s.parse()
        .map_err(|_| CollabError::Serde(format!("bad document id {s}")))
}
fn parse_fileref(s: &str) -> Result<FileRefId> {
    s.parse()
        .map_err(|_| CollabError::Serde(format!("bad file ref id {s}")))
}
fn parse_transfer(s: &str) -> Result<TransferId> {
    s.parse()
        .map_err(|_| CollabError::Serde(format!("bad transfer id {s}")))
}
fn parse_call(s: &str) -> Result<CallId> {
    s.parse()
        .map_err(|_| CollabError::Serde(format!("bad call id {s}")))
}

fn summarize(kind_tag: &str) -> String {
    kind_tag.replace('_', " ")
}
