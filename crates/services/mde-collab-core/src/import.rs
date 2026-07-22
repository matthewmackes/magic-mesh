//! The **pre-cutover migration importer** (WL-FUNC-011, U3).
//!
//! WL-FUNC-011 replaces seven legacy surfaces with one Communications suite
//! backed by this collab stack. The legacy state those surfaces left on disk —
//! the mde-chat per-conversation ring logs and the mde-editor autosave config —
//! must be carried *forward* into the collab model so a node that upgrades keeps
//! its history. This module is that one-shot, **idempotent**, re-run-safe bridge.
//!
//! # What it imports
//!
//! * **Chat ring history.** Each `<workgroup>/<host>/chat/out/<key>.json` file is
//!   an mde-chat ring (a JSON array of signed messages, capped at 500 — lock 8).
//!   For each conversation `key` the importer unions every host's ring (the same
//!   import-union the chat worker does), bootstraps a matching collab space
//!   ([`SpaceCreated`](CollabEventKind::SpaceCreated) + one
//!   [`MemberJoined`](CollabEventKind::MemberJoined) per participant, so the
//!   projection materializes it — an orphan space with no `SpaceCreated` writes
//!   nothing), and authors each ring message as a durable
//!   [`MessagePosted`](CollabEventKind::MessagePosted) event attributed to its
//!   original sender. Messages already evicted past the 500-cap are an accepted,
//!   unrecoverable boundary.
//! * **Editor autosave.** `<config>/mcnf/editor-egui.json` is read; each editor
//!   document it carries becomes a [`DocumentCreated`](CollabEventKind::DocumentCreated)
//!   in the migration's documents space. The shipping file holds only the
//!   autosave *preference* (no document bodies), so the honest live outcome is
//!   [`EditorImport::NothingToMigrate`]; a missing file is
//!   [`EditorImport::SourceMissing`]. Both are clean no-ops.
//!
//! # Idempotency (the re-run-safe contract)
//!
//! Every authored event's [`SpaceId`]/[`EventId`]/[`DocumentId`] is **derived
//! deterministically** from its stable source identity (the conversation key, the
//! ring message id, the document title) via the same SHA-256 → UUID mapping the
//! collab worker uses for its system space. Ed25519 signing is itself
//! deterministic, so a re-import mints byte-identical envelopes; the durable
//! [`ImportMap`] additionally records every source id it has already imported, so
//! a re-run skips finished work without re-authoring and the [`EventSink`]
//! (idempotent by [`EventId`]) never writes a duplicate line. The net effect:
//! importing the same source twice produces the exact same target state, with
//! zero duplicate events.
//!
//! # Purity + seams
//!
//! In the spirit of the rest of this crate, time is injected and both I/O
//! boundaries are traits a test backs with memory: the signer ([`EventSigner`])
//! and the [`EventSink`] the authored events land in ([`MemorySink`] for tests,
//! [`LogSink`] for the real Syncthing-replicable actor logs the worker converges
//! from). The source readers tolerate a missing/half-synced/corrupt file exactly
//! as the chat union does — a bad file is skipped, never fatal.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use mde_collab_types::event::CollabEventKind;
use mde_collab_types::ids::{DocumentId, EventId, SpaceId};
use mde_collab_types::value::{sha256_hex, MessageBody};
use mde_collab_types::{ActorClock, ActorId, CollabEventEnvelope, SpaceKind, SpaceRole};

use crate::error::Result;
use crate::log::{ActorLog, FileActorLog};
use crate::signer::EventSigner;

/// The current [`ImportMap`] schema version. Bump if the persisted shape changes.
pub const IMPORT_MAP_VERSION: u32 = 1;

/// Domain-separation prefixes for the deterministic id derivation, so a chat
/// message id, a space bootstrap marker, and a document title can never collide
/// into the same UUID, and none collides with the worker's own
/// `system_space_id` (which hashes the bare actor string).
const SEED_SPACE: &str = "wl-func-011-import|space|";
const SEED_EVENT: &str = "wl-func-011-import|event|";
const SEED_DOC: &str = "wl-func-011-import|document|";

/// The stable conversation key of the migration's editor-documents space (the
/// one space every imported editor document is linked into).
const EDITOR_SPACE_KEY: &str = "__editor_documents__";

// ───────────────────────────── durable idempotency map ─────────────────────

/// The durable source → target map that makes a re-import a no-op.
///
/// It records, per stable source id (a ring message id, a space/member bootstrap
/// marker, a document key), the [`EventId`] the importer minted for it. Persisted
/// as JSON next to the import so a re-run across a process restart still skips
/// finished work. Deterministic ids mean the map is belt-and-braces (the
/// [`EventSink`] dedups by id anyway), but it lets the importer *skip* re-signing
/// + re-writing already-imported facts and report honest counts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportMap {
    /// The schema version this map was written under.
    #[serde(default)]
    pub version: u32,
    /// source id → the target [`EventId`] it was imported as.
    #[serde(default)]
    pub imported: BTreeMap<String, EventId>,
    /// Whether the editor autosave source has been examined at least once.
    #[serde(default)]
    pub editor_scanned: bool,
}

impl ImportMap {
    /// A fresh, empty map at the current [`IMPORT_MAP_VERSION`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: IMPORT_MAP_VERSION,
            imported: BTreeMap::new(),
            editor_scanned: false,
        }
    }

    /// Load the durable map from `path`, or a fresh empty map when the file is
    /// absent (the first import). A corrupt file is an error the caller sees —
    /// silently discarding a map would risk re-importing duplicates.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read or parsed.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// Persist the map to `path` (atomic tmp-write + rename, creating parents) so
    /// a crash mid-write never leaves a torn map that would re-import duplicates.
    ///
    /// # Errors
    /// Returns an error if the directory cannot be created or the write fails.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self)?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, body.as_bytes())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Whether `source_id` has already been imported.
    #[must_use]
    pub fn contains(&self, source_id: &str) -> bool {
        self.imported.contains_key(source_id)
    }

    /// Record that `source_id` was imported as `event`.
    fn record(&mut self, source_id: String, event: EventId) {
        self.imported.insert(source_id, event);
    }
}

// ───────────────────────────── the event sink ──────────────────────────────

/// Where authored, signed migration events land. Idempotent by [`EventId`]:
/// writing an event already present is a no-op that returns `false`, so a re-run
/// never duplicates.
pub trait EventSink {
    /// Persist `env` if its [`EventId`] is not already present. Returns `true`
    /// when newly written, `false` when it was already there.
    ///
    /// # Errors
    /// Returns an error on an I/O or serialization failure.
    fn write_event(&mut self, env: &CollabEventEnvelope) -> Result<bool>;
}

/// An in-memory sink (tests + a dry run): deduplicated + kept in insertion order.
#[derive(Debug, Default)]
pub struct MemorySink {
    seen: BTreeSet<EventId>,
    order: Vec<CollabEventEnvelope>,
}

impl MemorySink {
    /// A fresh, empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every event written, in write order.
    #[must_use]
    pub fn events(&self) -> &[CollabEventEnvelope] {
        &self.order
    }

    /// How many distinct events the sink holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether the sink is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

impl EventSink for MemorySink {
    fn write_event(&mut self, env: &CollabEventEnvelope) -> Result<bool> {
        if self.seen.insert(env.event_id) {
            self.order.push(env.clone());
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// The production sink: the Syncthing-replicable [`FileActorLog`] tree at
/// `<root>/<space>/<actor>.jsonl` — exactly the actor-log root the collab worker
/// reads on boot (`backfill_logs`), so an imported log converges + projects like
/// any replicated peer log. Opens (and caches) one log per `(space, actor)` on
/// demand; the log's own idempotent append makes a re-import a no-op.
#[derive(Debug)]
pub struct LogSink {
    root: PathBuf,
    logs: BTreeMap<(SpaceId, ActorId), FileActorLog>,
}

impl LogSink {
    /// A sink writing actor logs beneath `root` (the worker's
    /// `<workgroup>/collab/logs` actor-log root).
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            logs: BTreeMap::new(),
        }
    }
}

impl EventSink for LogSink {
    fn write_event(&mut self, env: &CollabEventEnvelope) -> Result<bool> {
        let key = (env.space_id, env.actor.clone());
        // Open-and-cache on the miss path only; the borrow from the hit path does
        // not escape the match (its value is the `append` `Result`), so re-using
        // `self.logs` on the miss path is sound.
        match self.logs.get_mut(&key) {
            Some(log) => log.append(env),
            None => {
                let mut log = FileActorLog::open(&self.root, env.space_id, &env.actor)?;
                let appended = log.append(env);
                self.logs.insert(key, log);
                appended
            }
        }
    }
}

// ───────────────────────────── report shapes ───────────────────────────────

/// The outcome of the editor-autosave leg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorImport {
    /// The source file does not exist — a clean no-op.
    SourceMissing,
    /// The source was read but carried no document to migrate — a clean no-op
    /// (the honest live outcome, since `editor-egui.json` is a preference file).
    NothingToMigrate,
    /// `count` editor documents were authored as `DocumentCreated` events.
    Migrated {
        /// How many documents were newly authored.
        count: usize,
    },
}

/// A tally of one [`Importer`] run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Distinct collab spaces bootstrapped (found-or-created).
    pub spaces: usize,
    /// Ring messages newly authored as `MessagePosted` events.
    pub messages_imported: usize,
    /// Ring messages skipped because the durable map already had them.
    pub messages_skipped: usize,
    /// Editor documents newly authored as `DocumentCreated` events.
    pub documents_imported: usize,
}

// ───────────────────────────── the importer ────────────────────────────────

/// The one-shot, idempotent migration importer.
///
/// Holds the injected [`EventSigner`] (this node's identity key — the same key +
/// pattern the collab worker signs with) and the migrating node's [`ActorId`]
/// (the owner it stamps on every bootstrapped space). Imported *messages* keep
/// their original sender as the event actor for faithful attribution; the
/// signature carries this node's key, and binding a key to an identity is the
/// trust layer's job, not a lone envelope's (the envelope contract's rule).
pub struct Importer<'a, S: EventSigner> {
    signer: &'a S,
    self_actor: ActorId,
}

impl<'a, S: EventSigner> Importer<'a, S> {
    /// An importer signing with `signer`, owning bootstrapped spaces as
    /// `self_actor`.
    #[must_use]
    pub fn new(signer: &'a S, self_actor: impl Into<ActorId>) -> Self {
        Self {
            signer,
            self_actor: self_actor.into(),
        }
    }

    /// Import every chat ring beneath `chat_root` (a `<host>/chat/out/<key>.json`
    /// tree) into the collab model, writing authored events to `sink` and
    /// recording each in `map`. Idempotent: a re-run over the same source writes
    /// no new events.
    ///
    /// # Errors
    /// Returns an error only on a sink write failure; unreadable/corrupt source
    /// files are tolerated (skipped) like the chat import-union.
    pub fn import_chat_root(
        &self,
        chat_root: &Path,
        map: &mut ImportMap,
        sink: &mut dyn EventSink,
        now_unix_ms: i64,
    ) -> Result<ImportReport> {
        let mut report = ImportReport::default();
        // key -> unioned, id-deduplicated, time-ordered messages across hosts.
        let conversations = read_chat_conversations(chat_root);
        for (key, messages) in conversations {
            if messages.is_empty() {
                continue;
            }
            report.spaces += 1;
            self.import_conversation(&key, &messages, map, sink, now_unix_ms, &mut report)?;
        }
        Ok(report)
    }

    /// Import the editor autosave at `editor_json_path` into the collab Documents
    /// model. A missing file is [`EditorImport::SourceMissing`]; a file with no
    /// documents is [`EditorImport::NothingToMigrate`] — both clean no-ops.
    ///
    /// # Errors
    /// Returns an error only on a sink write failure.
    pub fn import_editor_autosave(
        &self,
        editor_json_path: &Path,
        map: &mut ImportMap,
        sink: &mut dyn EventSink,
        now_unix_ms: i64,
    ) -> Result<EditorImport> {
        map.editor_scanned = true;
        let Some(source) = read_editor_autosave(editor_json_path) else {
            return Ok(EditorImport::SourceMissing);
        };
        if source.documents.is_empty() {
            return Ok(EditorImport::NothingToMigrate);
        }
        let space = space_id_for(EDITOR_SPACE_KEY);
        self.bootstrap_space(
            space,
            EDITOR_SPACE_KEY,
            SpaceKind::Project,
            "Editor documents",
            &BTreeSet::new(),
            now_unix_ms,
            map,
            sink,
        )?;
        let mut count = 0_usize;
        for (idx, doc) in source.documents.iter().enumerate() {
            // A stable per-document source key: the caller-supplied id if any,
            // else the title, else the ordinal — so a re-run maps to the same doc.
            let doc_source = doc
                .id
                .clone()
                .filter(|s| !s.is_empty())
                .or_else(|| (!doc.title.is_empty()).then(|| doc.title.clone()))
                .unwrap_or_else(|| format!("doc-{idx}"));
            let source_id = format!("doc|{doc_source}");
            if map.contains(&source_id) {
                continue;
            }
            let document = DocumentId::from_seed(&doc_source);
            let event_id = event_id_for(&source_id);
            let title = if doc.title.is_empty() {
                doc_source.clone()
            } else {
                doc.title.clone()
            };
            let env = self.author(
                event_id,
                space,
                self.self_actor.clone(),
                ActorClock::at(clamp_ms(now_unix_ms), 0),
                now_unix_ms,
                CollabEventKind::DocumentCreated { document, title },
            );
            if sink.write_event(&env)? {
                count += 1;
            }
            map.record(source_id, event_id);
        }
        Ok(if count == 0 {
            EditorImport::NothingToMigrate
        } else {
            EditorImport::Migrated { count }
        })
    }

    /// Bootstrap + backfill one conversation's collab space.
    #[allow(clippy::too_many_arguments)]
    fn import_conversation(
        &self,
        key: &str,
        messages: &[RingMessage],
        map: &mut ImportMap,
        sink: &mut dyn EventSink,
        now_unix_ms: i64,
        report: &mut ImportReport,
    ) -> Result<()> {
        let space = space_id_for(key);
        // Participants = every distinct sender in the ring.
        let participants: BTreeSet<ActorId> = messages
            .iter()
            .filter(|m| !m.sender.is_empty())
            .map(|m| ActorId::new(m.sender.clone()))
            .collect();
        // A one-or-two-party conversation is a Direct; more is a Team room.
        let kind = if participants.len() <= 2 {
            SpaceKind::Direct
        } else {
            SpaceKind::Team
        };
        let created_ms = messages
            .iter()
            .map(|m| m.ts_unix_ms)
            .min()
            .unwrap_or(now_unix_ms);
        self.bootstrap_space(space, key, kind, key, &participants, created_ms, map, sink)?;

        for msg in messages {
            let source_id = format!("msg|{}", msg.id);
            if map.contains(&source_id) {
                report.messages_skipped += 1;
                continue;
            }
            let event_id = event_id_for(&source_id);
            let actor = if msg.sender.is_empty() {
                self.self_actor.clone()
            } else {
                ActorId::new(msg.sender.clone())
            };
            let env = self.author(
                event_id,
                space,
                actor,
                ActorClock::at(clamp_ms(msg.ts_unix_ms), 0),
                msg.ts_unix_ms,
                CollabEventKind::MessagePosted {
                    body: MessageBody::new(render_body(&msg.kind)),
                    thread: None,
                },
            );
            if sink.write_event(&env)? {
                report.messages_imported += 1;
            }
            map.record(source_id, event_id);
        }
        Ok(())
    }

    /// Author the `SpaceCreated` + one `MemberJoined` per participant that a
    /// space needs to materialize in the projection, idempotently (each keyed in
    /// the durable map). The migrating node joins as `Owner`; every other
    /// participant as a `Member`.
    #[allow(clippy::too_many_arguments)]
    fn bootstrap_space(
        &self,
        space: SpaceId,
        key: &str,
        kind: SpaceKind,
        name: &str,
        participants: &BTreeSet<ActorId>,
        created_ms: i64,
        map: &mut ImportMap,
        sink: &mut dyn EventSink,
    ) -> Result<()> {
        let created_source = format!("space|{key}");
        if !map.contains(&created_source) {
            let event_id = event_id_for(&created_source);
            let env = self.author(
                event_id,
                space,
                self.self_actor.clone(),
                ActorClock::at(clamp_ms(created_ms), 0),
                created_ms,
                CollabEventKind::SpaceCreated {
                    kind,
                    name: name.to_string(),
                },
            );
            sink.write_event(&env)?;
            map.record(created_source, event_id);
        }
        // The owner first, then every distinct participant (as a Member unless it
        // is the owner, who is already covered).
        let mut members: Vec<(ActorId, SpaceRole)> =
            vec![(self.self_actor.clone(), SpaceRole::Owner)];
        for p in participants {
            if p != &self.self_actor {
                members.push((p.clone(), SpaceRole::Member));
            }
        }
        for (actor, role) in members {
            let join_source = format!("member|{key}|{actor}");
            if map.contains(&join_source) {
                continue;
            }
            let event_id = event_id_for(&join_source);
            let env = self.author(
                event_id,
                space,
                actor.clone(),
                ActorClock::at(clamp_ms(created_ms), 0),
                created_ms,
                CollabEventKind::MemberJoined { actor, role },
            );
            sink.write_event(&env)?;
            map.record(join_source, event_id);
        }
        Ok(())
    }

    /// Assemble + sign one envelope with a caller-chosen (deterministic) id +
    /// clock. Signing is deterministic (Ed25519), so a re-import reproduces
    /// byte-identical bytes → the same signature → an idempotent write.
    fn author(
        &self,
        event_id: EventId,
        space: SpaceId,
        actor: ActorId,
        clock: ActorClock,
        created_unix_ms: i64,
        kind: CollabEventKind,
    ) -> CollabEventEnvelope {
        let mut env =
            CollabEventEnvelope::new(event_id, space, actor, clock, created_unix_ms, kind);
        self.signer.sign(&mut env);
        env
    }
}

// ───────────────────────────── deterministic ids ───────────────────────────

/// Format a 32-hex-nibble prefix of `hex` as a canonical `8-4-4-4-12` UUID
/// string (the exact shape the collab worker's `system_space_id` produces, so
/// this crate keeps no direct `uuid` dep and the mapping stays consistent).
fn hex_to_uuid_string(hex: &str) -> String {
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32],
    )
}

/// The deterministic [`SpaceId`] for a conversation key.
#[must_use]
fn space_id_for(key: &str) -> SpaceId {
    let hex = sha256_hex(format!("{SEED_SPACE}{key}").as_bytes());
    hex_to_uuid_string(&hex)
        .parse()
        .unwrap_or_else(|_| SpaceId::nil())
}

/// The deterministic [`EventId`] for a stable source id.
#[must_use]
fn event_id_for(source_id: &str) -> EventId {
    let hex = sha256_hex(format!("{SEED_EVENT}{source_id}").as_bytes());
    hex_to_uuid_string(&hex)
        .parse()
        .unwrap_or_else(|_| EventId::nil())
}

/// A small extension so a [`DocumentId`] can be minted deterministically from a
/// source key with the crate's no-`uuid`-dep UUID formatting.
trait FromSeed {
    /// Derive a stable id from `seed`.
    fn from_seed(seed: &str) -> Self;
}

impl FromSeed for DocumentId {
    fn from_seed(seed: &str) -> Self {
        let hex = sha256_hex(format!("{SEED_DOC}{seed}").as_bytes());
        hex_to_uuid_string(&hex)
            .parse()
            .unwrap_or_else(|_| Self::nil())
    }
}

/// Clamp an injected epoch-ms into the clock's `u64` wall component (a negative
/// pre-epoch stamp floors to 0 — the clock never runs backwards).
const fn clamp_ms(ms: i64) -> u64 {
    if ms < 0 {
        0
    } else {
        ms as u64
    }
}

// ───────────────────────────── source readers ──────────────────────────────

/// One mde-chat ring message (a decode mirror of `mde_chat::Message`, so this
/// crate takes no dependency on the legacy chat crate it is migrating *off* of).
/// Unknown fields (`signature`, …) are ignored — we re-author under this node's
/// signature, and the original signature has no meaning in the collab log.
#[derive(Debug, Clone, Deserialize)]
struct RingMessage {
    id: String,
    #[serde(default)]
    sender: String,
    #[serde(default)]
    ts_unix_ms: i64,
    kind: RingKind,
}

/// The mde-chat message kinds we render into a collab [`MessageBody`]. External-
/// tagged `snake_case`, matching the on-disk wire tags.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RingKind {
    /// A human-typed line.
    Text(String),
    /// A clipboard copy (we carry the preview forward; the full clip re-attaches
    /// via the dedicated clipboard lane, not here).
    Clipboard {
        /// The short one-line preview.
        preview: String,
        /// The full clip payload (unused here — it rides the clipboard lane).
        #[allow(dead_code)]
        full: String,
    },
    /// A folded alert card.
    Alert {
        /// The alert source flag.
        #[serde(default)]
        flag: String,
        /// The alert's string fields (title/summary/host/…).
        #[serde(default)]
        fields: BTreeMap<String, String>,
    },
    /// A file offer.
    File {
        /// The file name.
        name: String,
        /// The file size in bytes.
        #[serde(default)]
        size_bytes: u64,
    },
    /// A "start a call" hand-off.
    CallAction {
        /// The dialed host.
        target_host: String,
    },
    /// A "remote control" hand-off.
    RemoteAction {
        /// The remote-desktop host.
        target_host: String,
    },
}

/// Render an mde-chat message kind into the Markdown body of a collab message,
/// preserving the substance of the seven-surface history.
fn render_body(kind: &RingKind) -> String {
    match kind {
        RingKind::Text(t) => t.clone(),
        RingKind::Clipboard { preview, .. } => format!("copied a clipboard item: `{preview}`"),
        RingKind::Alert { flag, fields } => {
            let summary = fields
                .get("summary")
                .or_else(|| fields.get("title"))
                .or_else(|| fields.get("body"))
                .map_or("", String::as_str);
            if summary.is_empty() {
                format!("alert ({flag})")
            } else {
                format!("alert ({flag}): {summary}")
            }
        }
        RingKind::File { name, size_bytes } => {
            format!("shared file **{name}** ({size_bytes} bytes)")
        }
        RingKind::CallAction { target_host } => format!("started a call to {target_host}"),
        RingKind::RemoteAction { target_host } => {
            format!("opened a remote desktop to {target_host}")
        }
    }
}

/// Read + union every host's `<host>/chat/out/<key>.json` ring beneath
/// `chat_root`, keyed by conversation `key`. Within a key the messages are
/// deduplicated by id and sorted by `(ts, id)` — the same canonical order the
/// chat ring keeps — so a live copy and a Syncthing backfill fold identically.
/// A missing root / unreadable dir / corrupt file yields nothing for that path.
fn read_chat_conversations(chat_root: &Path) -> BTreeMap<String, Vec<RingMessage>> {
    // key -> (id -> message), so cross-host duplicates collapse by id.
    let mut by_key: BTreeMap<String, BTreeMap<String, RingMessage>> = BTreeMap::new();
    let Ok(hosts) = std::fs::read_dir(chat_root) else {
        return BTreeMap::new();
    };
    for host in hosts.flatten() {
        let out_dir = host.path().join("chat").join("out");
        let Ok(files) = std::fs::read_dir(&out_dir) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            let Some(key) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".json"))
            else {
                continue;
            };
            for msg in read_ring(&path) {
                by_key
                    .entry(key.to_string())
                    .or_default()
                    .entry(msg.id.clone())
                    .or_insert(msg);
            }
        }
    }
    by_key
        .into_iter()
        .map(|(key, msgs)| {
            let mut v: Vec<RingMessage> = msgs.into_values().collect();
            v.sort_by(|a, b| (a.ts_unix_ms, &a.id).cmp(&(b.ts_unix_ms, &b.id)));
            (key, v)
        })
        .collect()
}

/// Decode one ring file as a message vec; a missing/corrupt file is empty (the
/// union tolerates a half-synced or absent ring, exactly as the chat worker's
/// `read_log` does). Decoded per-entry so a single unrecognized message never
/// discards the whole ring — it is skipped, the rest import.
fn read_ring(path: &Path) -> Vec<RingMessage> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(values) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else {
        return Vec::new();
    };
    values
        .into_iter()
        .filter_map(|v| serde_json::from_value::<RingMessage>(v).ok())
        .collect()
}

/// The editor autosave file's decoded shape. The shipping file carries only the
/// autosave *preference*; the optional `documents` array is a forward-compatible
/// seam so a richer future autosave (open buffers) migrates without a code
/// change. Absent → no documents → nothing to migrate.
#[derive(Debug, Clone, Default, Deserialize)]
struct EditorAutosave {
    #[serde(default)]
    documents: Vec<EditorDocument>,
}

/// One editor document carried in the autosave file (forward-compat).
#[derive(Debug, Clone, Deserialize)]
struct EditorDocument {
    /// The stable document id, when the source has one.
    #[serde(default)]
    id: Option<String>,
    /// The document title.
    #[serde(default)]
    title: String,
}

/// Read + decode the editor autosave file, or `None` when it is absent/corrupt.
fn read_editor_autosave(path: &Path) -> Option<EditorAutosave> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<EditorAutosave>(&data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signer::Ed25519Signer;
    use crate::CollabEngine;

    fn signer() -> Ed25519Signer {
        // A fixed seed → deterministic signatures, so a re-import is byte-stable.
        Ed25519Signer::from_seed([7_u8; 32])
    }

    /// Write a chat ring file at `<root>/<host>/chat/out/<key>.json`.
    fn write_ring(root: &Path, host: &str, key: &str, body: &str) {
        let dir = root.join(host).join("chat").join("out");
        std::fs::create_dir_all(&dir).expect("mkdir out");
        std::fs::write(dir.join(format!("{key}.json")), body).expect("write ring");
    }

    /// A ring JSON array of `(id, sender, ts, text)` text messages.
    fn ring_json(entries: &[(&str, &str, i64, &str)]) -> String {
        let msgs: Vec<serde_json::Value> = entries
            .iter()
            .map(|(id, sender, ts, text)| {
                serde_json::json!({
                    "id": id,
                    "sender": sender,
                    "ts_unix_ms": ts,
                    "kind": { "text": text },
                })
            })
            .collect();
        serde_json::to_string(&msgs).expect("ring json")
    }

    #[test]
    fn imports_ring_messages_into_a_materialized_space() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write_ring(
            root,
            "eagle",
            "nyc3",
            &ring_json(&[
                ("01AAA", "eagle", 1_000, "hello nyc3"),
                ("01BBB", "nyc3", 2_000, "hi eagle"),
            ]),
        );
        let signer = signer();
        let importer = Importer::new(&signer, "eagle");
        let mut map = ImportMap::new();
        let mut sink = MemorySink::new();

        let report = importer
            .import_chat_root(root, &mut map, &mut sink, 9_999)
            .expect("import");
        assert_eq!(report.spaces, 1);
        assert_eq!(report.messages_imported, 2);
        assert_eq!(report.messages_skipped, 0);

        // Replay the authored events through a real engine → the two messages
        // materialize in the bootstrapped space (proves SpaceCreated was authored,
        // else the projection would drop the orphan messages).
        let mut engine = CollabEngine::in_memory("eagle").expect("engine");
        let outcome = engine.merge(sink.events().to_vec()).expect("merge");
        assert_eq!(outcome.dropped_invalid, 0, "every authored event verifies");
        let space = space_id_for("nyc3");
        let timeline = engine
            .projection()
            .conversation_timeline(space, None)
            .expect("timeline");
        assert_eq!(timeline.messages.len(), 2, "both ring messages projected");
        assert_eq!(timeline.messages[0].body, "hello nyc3");
        assert_eq!(timeline.messages[1].body, "hi eagle");
    }

    #[test]
    fn re_running_produces_no_duplicate_events() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write_ring(
            root,
            "eagle",
            "nyc3",
            &ring_json(&[("01AAA", "eagle", 1_000, "once")]),
        );
        let signer = signer();
        let importer = Importer::new(&signer, "eagle");
        let mut map = ImportMap::new();
        let mut sink = MemorySink::new();

        let first = importer
            .import_chat_root(root, &mut map, &mut sink, 5)
            .expect("first");
        assert_eq!(first.messages_imported, 1);
        let after_first = sink.len();

        // Second run over the same source + same map: nothing new is written and
        // the message is reported skipped.
        let second = importer
            .import_chat_root(root, &mut map, &mut sink, 6)
            .expect("second");
        assert_eq!(second.messages_imported, 0, "no new messages");
        assert_eq!(second.messages_skipped, 1, "the message is skipped");
        assert_eq!(sink.len(), after_first, "sink gained no events on re-run");
    }

    #[test]
    fn re_running_with_a_fresh_map_still_dedups_at_the_sink() {
        // Even if the durable map were lost, the deterministic ids + idempotent
        // sink still prevent duplicates (belt and braces).
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write_ring(
            root,
            "eagle",
            "nyc3",
            &ring_json(&[("01AAA", "eagle", 1_000, "once")]),
        );
        let signer = signer();
        let importer = Importer::new(&signer, "eagle");
        let mut sink = MemorySink::new();

        let mut map1 = ImportMap::new();
        importer
            .import_chat_root(root, &mut map1, &mut sink, 5)
            .expect("first");
        let n = sink.len();
        // Fresh map (simulating a lost idempotency map) → same deterministic ids.
        let mut map2 = ImportMap::new();
        importer
            .import_chat_root(root, &mut map2, &mut sink, 5)
            .expect("second");
        assert_eq!(sink.len(), n, "the idempotent sink absorbed the replay");
    }

    #[test]
    fn missing_chat_root_is_a_clean_no_op() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        let signer = signer();
        let importer = Importer::new(&signer, "eagle");
        let mut map = ImportMap::new();
        let mut sink = MemorySink::new();
        let report = importer
            .import_chat_root(&missing, &mut map, &mut sink, 1)
            .expect("no-op");
        assert_eq!(report, ImportReport::default());
        assert!(sink.is_empty());
    }

    #[test]
    fn cross_host_rings_union_and_dedup_by_id() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        // The same message id appears in two hosts' logs (a live copy + backfill);
        // it must collapse to one imported event.
        write_ring(
            root,
            "eagle",
            "nyc3",
            &ring_json(&[("SHARED", "eagle", 1, "hi")]),
        );
        write_ring(
            root,
            "nyc3",
            "nyc3",
            &ring_json(&[("SHARED", "eagle", 1, "hi")]),
        );
        let signer = signer();
        let importer = Importer::new(&signer, "eagle");
        let mut map = ImportMap::new();
        let mut sink = MemorySink::new();
        let report = importer
            .import_chat_root(root, &mut map, &mut sink, 1)
            .expect("import");
        assert_eq!(report.spaces, 1);
        assert_eq!(
            report.messages_imported, 1,
            "the duplicate id folded to one"
        );
    }

    #[test]
    fn log_sink_round_trips_and_is_idempotent_on_disk() {
        let src = tempfile::tempdir().expect("src");
        let logs = tempfile::tempdir().expect("logs");
        write_ring(
            src.path(),
            "eagle",
            "nyc3",
            &ring_json(&[("01AAA", "eagle", 1_000, "durable")]),
        );
        let signer = signer();
        let importer = Importer::new(&signer, "eagle");
        let mut map = ImportMap::new();

        {
            let mut sink = LogSink::new(logs.path());
            importer
                .import_chat_root(src.path(), &mut map, &mut sink, 5)
                .expect("first");
        }
        // A second, independent sink + fresh map re-reads the on-disk logs and
        // must append nothing new (FileActorLog dedups by event id on load).
        let mut map2 = ImportMap::new();
        {
            let mut sink = LogSink::new(logs.path());
            importer
                .import_chat_root(src.path(), &mut map2, &mut sink, 5)
                .expect("second");
        }
        // Read the space's actor log back and confirm exactly one MessagePosted.
        let space = space_id_for("nyc3");
        let log = FileActorLog::open(logs.path(), space, &ActorId::new("eagle")).expect("open");
        let posted = log
            .read_all()
            .expect("read")
            .into_iter()
            .filter(|e| matches!(e.kind, CollabEventKind::MessagePosted { .. }))
            .count();
        assert_eq!(posted, 1, "no duplicate message line after two imports");
    }

    #[test]
    fn import_map_persists_and_reloads() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("state").join("import-map.json");
        let mut map = ImportMap::new();
        map.record("msg|01AAA".to_string(), event_id_for("msg|01AAA"));
        map.save(&path).expect("save");
        let back = ImportMap::load(&path).expect("load");
        assert!(back.contains("msg|01AAA"));
        assert_eq!(back.version, IMPORT_MAP_VERSION);
        // A missing map file loads as empty (the first import).
        let fresh = ImportMap::load(&tmp.path().join("absent.json")).expect("load-absent");
        assert!(fresh.imported.is_empty());
    }

    #[test]
    fn missing_editor_source_is_source_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let signer = signer();
        let importer = Importer::new(&signer, "eagle");
        let mut map = ImportMap::new();
        let mut sink = MemorySink::new();
        let out = importer
            .import_editor_autosave(&tmp.path().join("editor-egui.json"), &mut map, &mut sink, 1)
            .expect("editor");
        assert_eq!(out, EditorImport::SourceMissing);
        assert!(sink.is_empty());
        assert!(map.editor_scanned, "the scan was recorded even when absent");
    }

    #[test]
    fn prefs_only_editor_file_is_nothing_to_migrate() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("editor-egui.json");
        // The real shipping file: just the autosave preference, no documents.
        std::fs::write(&path, r#"{"enabled":true,"idle_secs":2.0}"#).expect("write");
        let signer = signer();
        let importer = Importer::new(&signer, "eagle");
        let mut map = ImportMap::new();
        let mut sink = MemorySink::new();
        let out = importer
            .import_editor_autosave(&path, &mut map, &mut sink, 1)
            .expect("editor");
        assert_eq!(out, EditorImport::NothingToMigrate);
        assert!(sink.is_empty());
    }

    #[test]
    fn editor_documents_import_and_re_run_is_a_no_op() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("editor-egui.json");
        std::fs::write(
            &path,
            r#"{"enabled":true,"documents":[{"id":"d1","title":"Runbook"}]}"#,
        )
        .expect("write");
        let signer = signer();
        let importer = Importer::new(&signer, "eagle");
        let mut map = ImportMap::new();
        let mut sink = MemorySink::new();

        let first = importer
            .import_editor_autosave(&path, &mut map, &mut sink, 1)
            .expect("first");
        assert_eq!(first, EditorImport::Migrated { count: 1 });

        let second = importer
            .import_editor_autosave(&path, &mut map, &mut sink, 1)
            .expect("second");
        assert_eq!(
            second,
            EditorImport::NothingToMigrate,
            "re-run adds nothing"
        );

        // The document materializes when replayed through an engine.
        let mut engine = CollabEngine::in_memory("eagle").expect("engine");
        engine.merge(sink.events().to_vec()).expect("merge");
        let space = space_id_for(EDITOR_SPACE_KEY);
        let docs = engine
            .projection()
            .document_sessions(Some(space))
            .expect("docs");
        assert_eq!(docs.sessions.len(), 1);
        assert_eq!(docs.sessions[0].title, "Runbook");
    }
}
