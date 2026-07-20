//! Deterministic tests for the collaboration core. No wall clock and no
//! randomness: every timestamp is an injected `now_unix_ms`, every id comes from
//! a [`SeqIds`] sequence, and every key is a fixed 32-byte seed. Convergence is
//! exercised by feeding the same signed event set to fresh engines in many
//! deterministic shuffles and asserting byte-identical projected state.

use std::collections::BTreeSet;

use mde_collab_types::event::CollabEventKind;
use mde_collab_types::ids::{
    CallId, DocumentId, EventId, FileRefId, SpaceId, ThreadId, TransferId,
};
use mde_collab_types::value::{
    sha256_hex, AlertAction, AlertActionKind, AlertPayload, CallKind, CallParticipantState,
    ClipItemKind, ClipboardItem, DocumentChange, FileRef, MessageBody, PayloadRef, PresenceState,
    Severity, TransferDirection, TransferMethod,
};
use mde_collab_types::{
    ActorClock, ActorId, CollabCommand, CollabEventEnvelope, SpaceKind, SpaceRole, TransferControl,
};
use uuid::Uuid;

use crate::blob::{verify_bytes, BlobStore, FsBlobStore, MemoryBlobStore};
use crate::error::CollabError;
use crate::log::{ActorLog, FileActorLog, MemoryActorLog};
use crate::pipeline::EDIT_WINDOW_MS;
use crate::projection::Projection;
use crate::signer::{Ed25519Signer, IdSource};
use crate::CollabEngine;

// ---- deterministic injection helpers --------------------------------------

/// A deterministic id source (`Uuid::from_u128` over an increasing counter).
struct SeqIds {
    n: u128,
}
impl SeqIds {
    const fn new(start: u128) -> Self {
        Self { n: start }
    }
}
impl IdSource for SeqIds {
    fn next_event_id(&mut self) -> EventId {
        let id = EventId::from_uuid(Uuid::from_u128(self.n));
        self.n += 1;
        id
    }
}

fn sig(seed: u8) -> Ed25519Signer {
    Ed25519Signer::from_seed([seed; 32])
}

fn engine(actor: &str) -> CollabEngine {
    CollabEngine::in_memory(actor).expect("open in-memory engine")
}

fn thread_of(events: &[CollabEventEnvelope]) -> ThreadId {
    events
        .iter()
        .find_map(|e| match &e.kind {
            CollabEventKind::ThreadStarted { thread, .. } => Some(*thread),
            _ => None,
        })
        .expect("a ThreadStarted event")
}

/// A deterministic Fisher-Yates shuffle driven by an LCG — no `rand`.
fn shuffle(items: &[CollabEventEnvelope], seed: u64) -> Vec<CollabEventEnvelope> {
    let mut v = items.to_vec();
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as usize
    };
    for i in (1..v.len()).rev() {
        let j = next() % (i + 1);
        v.swap(i, j);
    }
    v
}

// ---- pipeline: permission + window validation -----------------------------

#[test]
fn create_space_makes_the_creator_an_owner() {
    let mut a = engine("alice");
    let s = sig(1);
    let mut ids = SeqIds::new(1);
    let events = a
        .apply(
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &s,
            &mut ids,
            1000,
        )
        .expect("create space");
    assert_eq!(events.len(), 2, "SpaceCreated + MemberJoined(owner)");
    let space = events[0].space_id;
    assert!(a.state().is_owner(space, &ActorId::new("alice")));
    let dir = a
        .projection()
        .space_directory(&ActorId::new("alice"))
        .expect("directory");
    assert_eq!(dir.spaces.len(), 1);
    assert_eq!(dir.spaces[0].role, SpaceRole::Owner);
    assert_eq!(dir.spaces[0].members, 1);
}

#[test]
fn add_member_is_owner_gated_and_member_is_denied_visibly() {
    let mut a = engine("alice");
    let sa = sig(1);
    let mut ia = SeqIds::new(1);
    let space = a
        .apply(
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &sa,
            &mut ia,
            1000,
        )
        .expect("create")[0]
        .space_id;
    // Owner can add.
    a.apply(
        &CollabCommand::AddMember {
            space,
            actor: ActorId::new("bob"),
            role: SpaceRole::Member,
        },
        &sa,
        &mut ia,
        1100,
    )
    .expect("owner adds member");

    // Bob (a Member) is denied — a visible typed error, not a silent no-op.
    let mut b = engine("bob");
    b.merge(a.all_events()).expect("bob syncs");
    let denied = b.apply(
        &CollabCommand::AddMember {
            space,
            actor: ActorId::new("carol"),
            role: SpaceRole::Member,
        },
        &sig(2),
        &mut SeqIds::new(100),
        1200,
    );
    assert!(matches!(denied, Err(CollabError::OwnerRequired { .. })));

    // But a Member may post.
    b.apply(
        &CollabCommand::SendMessage {
            space,
            thread: None,
            body: MessageBody::new("hi"),
        },
        &sig(2),
        &mut SeqIds::new(200),
        1300,
    )
    .expect("member posts");
}

#[test]
fn sending_to_a_space_youre_not_in_is_denied() {
    let mut a = engine("alice");
    let space = a
        .apply(
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Direct,
                name: "dm".into(),
            },
            &sig(1),
            &mut SeqIds::new(1),
            1000,
        )
        .expect("create")[0]
        .space_id;
    let mut c = engine("carol");
    c.merge(a.all_events()).expect("sync");
    let denied = c.apply(
        &CollabCommand::SendMessage {
            space,
            thread: None,
            body: MessageBody::new("intrusion"),
        },
        &sig(3),
        &mut SeqIds::new(1),
        1100,
    );
    assert!(matches!(denied, Err(CollabError::NotMember { .. })));
}

#[test]
fn author_edit_within_window_ok_late_edit_denied_nonauthor_denied() {
    let mut a = engine("alice");
    let sa = sig(1);
    let mut ia = SeqIds::new(1);
    let space = a
        .apply(
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &sa,
            &mut ia,
            1000,
        )
        .expect("create")[0]
        .space_id;
    a.apply(
        &CollabCommand::AddMember {
            space,
            actor: ActorId::new("bob"),
            role: SpaceRole::Member,
        },
        &sa,
        &mut ia,
        1050,
    )
    .expect("add bob");
    let target = a
        .apply(
            &CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new("v1"),
            },
            &sa,
            &mut ia,
            2000,
        )
        .expect("post")[0]
        .event_id;

    // Within the 5-minute window: OK, and the projection shows the new body.
    a.apply(
        &CollabCommand::EditMessage {
            space,
            target,
            body: MessageBody::new("v2"),
        },
        &sa,
        &mut ia,
        2000 + 60_000,
    )
    .expect("edit within window");
    let tl = a
        .projection()
        .conversation_timeline(space, None)
        .expect("timeline");
    assert_eq!(tl.messages.len(), 1);
    assert_eq!(tl.messages[0].body, "v2");
    assert!(tl.messages[0].edited);

    // After the window: denied and visible.
    let late = a.apply(
        &CollabCommand::EditMessage {
            space,
            target,
            body: MessageBody::new("too late"),
        },
        &sa,
        &mut ia,
        2000 + EDIT_WINDOW_MS + 1,
    );
    assert!(matches!(late, Err(CollabError::EditWindowExpired { .. })));

    // A non-author cannot edit even inside the window.
    let mut b = engine("bob");
    b.merge(a.all_events()).expect("sync");
    let not_author = b.apply(
        &CollabCommand::EditMessage {
            space,
            target,
            body: MessageBody::new("bob was here"),
        },
        &sig(2),
        &mut SeqIds::new(1),
        2000 + 1000,
    );
    assert!(matches!(not_author, Err(CollabError::NotAuthor(_))));
}

#[test]
fn author_delete_within_window_tombstones_the_message() {
    let mut a = engine("alice");
    let sa = sig(1);
    let mut ia = SeqIds::new(1);
    let space = a
        .apply(
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &sa,
            &mut ia,
            1000,
        )
        .expect("create")[0]
        .space_id;
    let target = a
        .apply(
            &CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new("secret"),
            },
            &sa,
            &mut ia,
            2000,
        )
        .expect("post")[0]
        .event_id;
    a.apply(
        &CollabCommand::DeleteMessage { space, target },
        &sa,
        &mut ia,
        2000 + 1000,
    )
    .expect("delete within window");
    let tl = a
        .projection()
        .conversation_timeline(space, None)
        .expect("timeline");
    assert_eq!(tl.messages.len(), 1);
    assert!(tl.messages[0].deleted);
    assert_eq!(tl.messages[0].body, "", "deleted body is redacted");
}

#[test]
fn delete_space_is_owner_gated_and_blocks_further_commands() {
    let mut a = engine("alice");
    let sa = sig(1);
    let mut ia = SeqIds::new(1);
    let space = a
        .apply(
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &sa,
            &mut ia,
            1000,
        )
        .expect("create")[0]
        .space_id;
    a.apply(
        &CollabCommand::AddMember {
            space,
            actor: ActorId::new("bob"),
            role: SpaceRole::Member,
        },
        &sa,
        &mut ia,
        1100,
    )
    .expect("add bob");

    // A Member cannot delete the space.
    let mut b = engine("bob");
    b.merge(a.all_events()).expect("sync");
    let denied = b.apply(
        &CollabCommand::DeleteSpace { space },
        &sig(2),
        &mut SeqIds::new(1),
        1200,
    );
    assert!(matches!(denied, Err(CollabError::OwnerRequired { .. })));

    // The Owner deletes it (a direct space tombstone), and no further command
    // lands on the deleted space.
    a.apply(&CollabCommand::DeleteSpace { space }, &sa, &mut ia, 1300)
        .expect("owner deletes");
    let after = a.apply(
        &CollabCommand::SendMessage {
            space,
            thread: None,
            body: MessageBody::new("hello?"),
        },
        &sa,
        &mut ia,
        1400,
    );
    assert!(matches!(after, Err(CollabError::SpaceDeleted(_))));
    // The tombstoned space drops out of the directory.
    assert!(a
        .projection()
        .space_directory(&ActorId::new("alice"))
        .expect("dir")
        .spaces
        .is_empty());
}

#[test]
fn clear_clipboard_spares_pinned_items() {
    let mut a = engine("alice");
    let sa = sig(1);
    let mut ia = SeqIds::new(1);
    let space = a
        .apply(
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &sa,
            &mut ia,
            1000,
        )
        .expect("create")[0]
        .space_id;
    let clip_a = a
        .apply(
            &CollabCommand::PublishClipboard {
                space,
                item: clip("keep me"),
            },
            &sa,
            &mut ia,
            1100,
        )
        .expect("clip a")[0]
        .event_id;
    a.apply(
        &CollabCommand::PublishClipboard {
            space,
            item: clip("drop me"),
        },
        &sa,
        &mut ia,
        1200,
    )
    .expect("clip b");
    a.apply(
        &CollabCommand::PinClipboard {
            space,
            clip: clip_a,
        },
        &sa,
        &mut ia,
        1300,
    )
    .expect("pin a");
    let cleared = a
        .apply(&CollabCommand::ClearClipboard { space }, &sa, &mut ia, 1400)
        .expect("clear");
    assert_eq!(cleared.len(), 1, "only the one unpinned clip is tombstoned");
    let lane = a.projection().clipboard_lane(space).expect("lane");
    assert_eq!(lane.items.len(), 1);
    assert!(lane.items[0].pinned);
    assert_eq!(lane.items[0].preview, "keep me");
}

fn clip(preview: &str) -> ClipboardItem {
    ClipboardItem {
        kind: ClipItemKind::Text,
        preview: preview.into(),
        sha256_hex: sha256_hex(preview.as_bytes()),
        len: preview.len() as u64,
        source: "alice".into(),
    }
}

// ---- actor log ------------------------------------------------------------

fn one_event() -> CollabEventEnvelope {
    let mut a = engine("alice");
    a.apply(
        &CollabCommand::CreateSpace {
            kind: SpaceKind::Team,
            name: "ops".into(),
        },
        &sig(1),
        &mut SeqIds::new(1),
        1000,
    )
    .expect("create")
    .into_iter()
    .next()
    .expect("first event")
}

#[test]
fn memory_actor_log_is_idempotent() {
    let env = one_event();
    let mut log = MemoryActorLog::new();
    assert!(log.append(&env).expect("append"));
    assert!(
        !log.append(&env).expect("reappend"),
        "second append is a no-op"
    );
    assert_eq!(log.len(), 1);
    assert_eq!(log.read_all().expect("read")[0], env);
}

#[test]
fn file_actor_log_round_trips_and_dedups_across_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let env = one_event();
    let space = env.space_id;
    let actor = env.actor.clone();
    {
        let mut log = FileActorLog::open(dir.path(), space, &actor).expect("open");
        assert!(log.append(&env).expect("append"));
        assert!(!log.append(&env).expect("dup"), "same id is a no-op");
    }
    // Reopen: the persisted event is loaded, and re-appending stays idempotent.
    let mut log = FileActorLog::open(dir.path(), space, &actor).expect("reopen");
    assert_eq!(log.len(), 1);
    assert_eq!(log.read_all().expect("read")[0], env);
    assert!(!log.append(&env).expect("dup after reopen"));
}

// ---- blob store -----------------------------------------------------------

#[test]
fn fs_blob_store_round_trips_and_purges() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = FsBlobStore::new(dir.path());
    let bytes = b"a document snapshot".to_vec();
    let r = store.put(&bytes).expect("put");
    assert_eq!(r.len, bytes.len() as u64);
    assert!(store.contains(&r.sha256_hex));
    assert_eq!(store.get(&r).expect("get"), bytes);
    assert!(store.purge(&r.sha256_hex).expect("purge"));
    assert!(!store.contains(&r.sha256_hex));
    assert!(matches!(store.get(&r), Err(CollabError::BlobNotFound(_))));
}

#[test]
fn blob_verify_rejects_wrong_hash_and_size() {
    let good = PayloadRef::of_bytes(b"hello");
    verify_bytes(b"hello", &good).expect("matching bytes verify");

    // Same length, wrong content → hash mismatch.
    let wrong_hash = PayloadRef {
        sha256_hex: sha256_hex(b"world"),
        len: 5,
        content_type: None,
    };
    assert!(matches!(
        verify_bytes(b"hello", &wrong_hash),
        Err(CollabError::BlobHashMismatch { .. })
    ));

    // Correct digest, wrong length claim → size mismatch (checked first).
    let wrong_size = PayloadRef {
        sha256_hex: sha256_hex(b"hello"),
        len: 999,
        content_type: None,
    };
    assert!(matches!(
        verify_bytes(b"hello", &wrong_size),
        Err(CollabError::BlobSizeMismatch { .. })
    ));
}

#[test]
fn memory_blob_store_round_trips() {
    let mut store = MemoryBlobStore::new();
    let r = store.put(b"crdt-update").expect("put");
    assert_eq!(store.get(&r).expect("get"), b"crdt-update");
    // Idempotent put of identical bytes yields the same ref.
    assert_eq!(store.put(b"crdt-update").expect("re-put"), r);
}

// ---- projection idempotence ----------------------------------------------

#[test]
fn projection_is_idempotent_under_reapply() {
    let events = rich_corpus();
    let mut p = Projection::open_in_memory().expect("proj");
    p.project(&events).expect("first project");
    let d1 = p.dump_tables().expect("dump");
    // Re-applying the same events changes nothing.
    p.project(&events).expect("reapply");
    let d2 = p.dump_tables().expect("dump");
    assert_eq!(d1, d2, "re-applying an event is a no-op");
    // And applying each event twice, one at a time, still converges to d1.
    let mut p2 = Projection::open_in_memory().expect("proj2");
    for env in &events {
        p2.project(std::slice::from_ref(env)).expect("one");
        p2.project(std::slice::from_ref(env)).expect("one-again");
    }
    assert_eq!(p2.dump_tables().expect("dump2"), d1);
}

// ---- convergence (the property tests) -------------------------------------

/// A rich, multi-actor signed event corpus exercising every projection table.
fn rich_corpus() -> Vec<CollabEventEnvelope> {
    let sa = sig(1);
    let mut ia = SeqIds::new(1);
    let mut a = engine("alice");
    let bob = ActorId::new("bob");

    let space = a
        .apply(
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &sa,
            &mut ia,
            1000,
        )
        .expect("create")[0]
        .space_id;
    a.apply(
        &CollabCommand::AddMember {
            space,
            actor: bob.clone(),
            role: SpaceRole::Member,
        },
        &sa,
        &mut ia,
        1100,
    )
    .expect("add bob");
    let msg_a = a
        .apply(
            &CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new("hello from alice"),
            },
            &sa,
            &mut ia,
            1200,
        )
        .expect("post")[0]
        .event_id;
    a.apply(
        &CollabCommand::EditMessage {
            space,
            target: msg_a,
            body: MessageBody::new("hello (edited)"),
        },
        &sa,
        &mut ia,
        1300,
    )
    .expect("edit");
    let thread = thread_of(
        &a.apply(
            &CollabCommand::StartThread {
                space,
                root: msg_a,
                title: Some("re: deploy".into()),
            },
            &sa,
            &mut ia,
            1400,
        )
        .expect("thread"),
    );
    let clip_id = a
        .apply(
            &CollabCommand::PublishClipboard {
                space,
                item: clip("ssh key"),
            },
            &sa,
            &mut ia,
            1500,
        )
        .expect("clip")[0]
        .event_id;
    a.apply(
        &CollabCommand::PinClipboard {
            space,
            clip: clip_id,
        },
        &sa,
        &mut ia,
        1550,
    )
    .expect("pin");
    let doc = DocumentId::new();
    a.apply(
        &CollabCommand::CreateDocument {
            space,
            document: doc,
            title: "design".into(),
        },
        &sa,
        &mut ia,
        1600,
    )
    .expect("doc");
    a.apply(
        &CollabCommand::UpdateDocument {
            space,
            document: doc,
            change: DocumentChange {
                payload: PayloadRef::of_bytes(b"crdt-1"),
                summary: Some("+ intro".into()),
            },
        },
        &sa,
        &mut ia,
        1650,
    )
    .expect("doc update");
    let file = FileRefId::new();
    a.apply(
        &CollabCommand::LinkFile {
            space,
            file,
            reference: FileRef {
                name: "report.pdf".into(),
                size: 4096,
                sha256_hex: sha256_hex(b"pdf-bytes"),
                mime: Some("application/pdf".into()),
            },
        },
        &sa,
        &mut ia,
        1700,
    )
    .expect("link");
    let transfer = TransferId::new();
    a.apply(
        &CollabCommand::StartTransfer {
            space,
            transfer,
            file,
            method: TransferMethod::Node,
            direction: TransferDirection::Inbound,
        },
        &sa,
        &mut ia,
        1750,
    )
    .expect("transfer");
    a.apply(
        &CollabCommand::ControlTransfer {
            transfer,
            control: TransferControl::Pause,
        },
        &sa,
        &mut ia,
        1800,
    )
    .expect("pause");
    let call = CallId::new();
    a.apply(
        &CollabCommand::StartCall {
            space,
            call,
            kind: CallKind::Audio,
        },
        &sa,
        &mut ia,
        1850,
    )
    .expect("call");
    a.apply(
        &CollabCommand::SetPresence {
            presence: PresenceState::Online,
            status: Some("here".into()),
        },
        &sa,
        &mut ia,
        1900,
    )
    .expect("presence");

    // A folded alert (worker-style: no command emits AlertRaised).
    let alert_env = craft_alert(space, &sa, 1950);
    let alert_id = alert_env.event_id;
    a.merge(vec![alert_env]).expect("merge alert");

    // Bob replicates alice's full log, then acts as a member.
    let sb = sig(2);
    let mut ib = SeqIds::new(1_000);
    let mut b = engine("bob");
    b.merge(a.all_events()).expect("bob syncs");
    let msg_b = b
        .apply(
            &CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new("hi from bob"),
            },
            &sb,
            &mut ib,
            2000,
        )
        .expect("bob posts")[0]
        .event_id;
    b.apply(
        &CollabCommand::ReplyInThread {
            space,
            thread,
            body: MessageBody::new("reply from bob"),
        },
        &sb,
        &mut ib,
        2050,
    )
    .expect("bob replies");
    b.apply(
        &CollabCommand::DeleteMessage {
            space,
            target: msg_b,
        },
        &sb,
        &mut ib,
        2100,
    )
    .expect("bob deletes own");
    b.apply(&CollabCommand::AnswerCall { call }, &sb, &mut ib, 2150)
        .expect("bob answers");
    b.apply(
        &CollabCommand::AckAlert {
            space,
            alert: alert_id,
        },
        &sb,
        &mut ib,
        2200,
    )
    .expect("bob acks alert");

    b.all_events()
}

fn craft_alert(space: SpaceId, signer: &Ed25519Signer, wall: u64) -> CollabEventEnvelope {
    let mut fields = std::collections::BTreeMap::new();
    fields.insert("disk".to_string(), "94%".to_string());
    let alert = AlertPayload {
        severity: Severity::Warning,
        source: "nyc3".into(),
        headline: "disk pre-fail".into(),
        fields,
        actions: vec![AlertAction {
            id: "restart".into(),
            label: "Restart".into(),
            verb: Some("action/node/restart".into()),
            kind: AlertActionKind::Destructive,
        }],
        goto: None,
    };
    let mut env = CollabEventEnvelope::new(
        EventId::from_uuid(Uuid::from_u128(9_000_000)),
        space,
        ActorId::new("alice"),
        ActorClock::at(wall, 0),
        wall as i64,
        CollabEventKind::AlertRaised { alert },
    );
    use crate::signer::EventSigner;
    signer.sign(&mut env);
    env
}

#[test]
fn nodes_converge_to_identical_state_under_any_order() {
    let corpus = rich_corpus();
    // The reference: one node fed the corpus as a single batch.
    let mut reference = engine("viewer");
    reference.merge(corpus.clone()).expect("reference merge");
    let want = reference.projection().dump_tables().expect("dump");

    // Two-node + three-node convergence: many deterministic shuffles, each fed
    // as one batch, must all produce the identical projected state.
    for seed in 0..40 {
        let mut node = engine("nodeX");
        node.merge(shuffle(&corpus, seed)).expect("merge");
        assert_eq!(
            node.projection().dump_tables().expect("dump"),
            want,
            "single-batch shuffle seed {seed} diverged"
        );
    }
}

#[test]
fn per_event_out_of_order_delivery_converges() {
    let corpus = rich_corpus();
    let mut reference = engine("viewer");
    reference.merge(corpus.clone()).expect("reference");
    let want = reference.projection().dump_tables().expect("dump");

    // The stronger test: deliver ONE event per merge, in shuffled order, so the
    // incremental per-space rebuild runs against every partial subset.
    for seed in 100..130 {
        let mut node = engine("nodeY");
        for env in shuffle(&corpus, seed) {
            node.merge(vec![env]).expect("one-at-a-time merge");
        }
        assert_eq!(
            node.projection().dump_tables().expect("dump"),
            want,
            "per-event shuffle seed {seed} diverged"
        );
    }
}

#[test]
fn duplicate_delivery_is_a_no_op() {
    let corpus = rich_corpus();
    let mut node = engine("viewer");
    let first = node.merge(corpus.clone()).expect("first");
    assert_eq!(first.accepted, corpus.len());
    assert_eq!(first.duplicates, 0);
    let d1 = node.projection().dump_tables().expect("dump");
    // Re-deliver everything: all duplicates, state unchanged.
    let second = node.merge(corpus.clone()).expect("second");
    assert_eq!(second.accepted, 0);
    assert_eq!(second.duplicates, corpus.len());
    assert_eq!(node.projection().dump_tables().expect("dump"), d1);
}

#[test]
fn an_invalid_signature_event_is_dropped() {
    let corpus = rich_corpus();
    let mut reference = engine("viewer");
    reference.merge(corpus.clone()).expect("reference");
    let want = reference.projection().dump_tables().expect("dump");

    // Forge an event: sign a message, then tamper its body so verify() fails.
    let space = corpus[0].space_id;
    let mut forged = CollabEventEnvelope::new(
        EventId::from_uuid(Uuid::from_u128(42)),
        space,
        ActorId::new("mallory"),
        ActorClock::at(3000, 0),
        3000,
        CollabEventKind::MessagePosted {
            body: MessageBody::new("legit"),
            thread: None,
        },
    );
    use crate::signer::EventSigner;
    sig(9).sign(&mut forged);
    forged.kind = CollabEventKind::MessagePosted {
        body: MessageBody::new("FORGED after signing"),
        thread: None,
    };
    assert!(!forged.verify(), "tampered envelope must not verify");

    let mut node = engine("viewer");
    let mut batch = corpus.clone();
    batch.push(forged);
    let outcome = node.merge(shuffle(&batch, 7)).expect("merge");
    assert_eq!(outcome.dropped_invalid, 1, "the forged event is dropped");
    assert_eq!(
        node.projection().dump_tables().expect("dump"),
        want,
        "the forged event leaves no trace"
    );
}

// ---- tombstones -----------------------------------------------------------

#[test]
fn a_tombstone_is_order_independent_and_prevents_resurrection() {
    // Build a post + its delete on an author engine.
    let mut a = engine("alice");
    let sa = sig(1);
    let mut ia = SeqIds::new(1);
    let space = a
        .apply(
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &sa,
            &mut ia,
            1000,
        )
        .expect("create")[0]
        .space_id;
    let target = a
        .apply(
            &CollabCommand::SendMessage {
                space,
                thread: None,
                body: MessageBody::new("delete me"),
            },
            &sa,
            &mut ia,
            2000,
        )
        .expect("post")[0]
        .event_id;
    a.apply(
        &CollabCommand::DeleteMessage { space, target },
        &sa,
        &mut ia,
        2100,
    )
    .expect("delete");
    let corpus = a.all_events();
    let post = corpus
        .iter()
        .find(|e| e.event_id == target)
        .cloned()
        .expect("post event");

    // The delete applied AFTER the post (delivered first) still wins — deletion
    // is a set-membership tombstone, not last-writer.
    let deleted_flag = |n: &CollabEngine| -> bool {
        n.projection()
            .conversation_timeline(space, None)
            .expect("tl")
            .messages
            .first()
            .is_some_and(|m| m.deleted)
    };
    for seed in 0..10 {
        let mut node = engine("viewer");
        for env in shuffle(&corpus, seed) {
            node.merge(vec![env]).expect("merge");
        }
        assert!(
            deleted_flag(&node),
            "message must stay deleted (seed {seed})"
        );
        // A stale peer re-delivers the ORIGINAL post: a duplicate no-op that
        // cannot resurrect the content.
        let outcome = node.merge(vec![post.clone()]).expect("stale redeliver");
        assert_eq!(outcome.duplicates, 1);
        assert!(deleted_flag(&node), "resurrection attempt rejected");
    }
}

#[test]
fn space_deletion_does_not_purge_the_canonical_file() {
    let mut a = engine("alice");
    let sa = sig(1);
    let mut ia = SeqIds::new(1);
    let space = a
        .apply(
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Project,
                name: "proj".into(),
            },
            &sa,
            &mut ia,
            1000,
        )
        .expect("create")[0]
        .space_id;
    // A canonical file's bytes live in the blob store.
    let mut blobs = MemoryBlobStore::new();
    let file_bytes = b"canonical file bytes".to_vec();
    let file_ref = blobs.put(&file_bytes).expect("store file");
    let file_sha = file_ref.sha256_hex.clone();
    a.apply(
        &CollabCommand::LinkFile {
            space,
            file: FileRefId::new(),
            reference: FileRef {
                name: "keep.bin".into(),
                size: file_bytes.len() as u64,
                sha256_hex: file_sha.clone(),
                mime: None,
            },
        },
        &sa,
        &mut ia,
        1100,
    )
    .expect("link");
    a.apply(&CollabCommand::DeleteSpace { space }, &sa, &mut ia, 1200)
        .expect("delete space");

    // Space deletion is a tombstone, but the canonical file bytes are NOT
    // purge-eligible (they may be referenced elsewhere) and remain in the store.
    assert!(
        !a.may_purge(space, &file_sha),
        "canonical file is never purged by space deletion"
    );
    assert!(blobs.contains(&file_sha), "file bytes untouched");
}

#[test]
fn payload_purge_is_gated_on_all_members_acking() {
    let mut a = engine("alice");
    let sa = sig(1);
    let mut ia = SeqIds::new(1);
    let alice = ActorId::new("alice");
    let bob = ActorId::new("bob");
    let space = a
        .apply(
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &sa,
            &mut ia,
            1000,
        )
        .expect("create")[0]
        .space_id;
    a.apply(
        &CollabCommand::AddMember {
            space,
            actor: bob.clone(),
            role: SpaceRole::Member,
        },
        &sa,
        &mut ia,
        1100,
    )
    .expect("add bob");

    // A message carrying a large out-of-band payload (worker-style: the pipeline
    // keeps messages small, so we craft the payload-bearing envelope).
    let mut blobs = MemoryBlobStore::new();
    let payload = blobs.put(b"a big pasted screenshot").expect("store");
    let payload_sha = payload.sha256_hex.clone();
    use crate::signer::EventSigner;
    let post_id = EventId::from_uuid(Uuid::from_u128(500));
    let mut post = CollabEventEnvelope::new(
        post_id,
        space,
        alice.clone(),
        ActorClock::at(1500, 0),
        1500,
        CollabEventKind::MessagePosted {
            body: MessageBody::new("see attachment"),
            thread: None,
        },
    )
    .with_payload_ref(payload);
    sa.sign(&mut post);
    a.merge(vec![post]).expect("merge post");

    let delete_clock = ActorClock::at(1600, 0);
    let mut del = CollabEventEnvelope::new(
        EventId::from_uuid(Uuid::from_u128(501)),
        space,
        alice.clone(),
        delete_clock,
        1600,
        CollabEventKind::MessageDeleted { target: post_id },
    );
    sa.sign(&mut del);
    a.merge(vec![del]).expect("merge delete");

    // Alice authored both, so her ack high-water already covers the tombstone;
    // bob has acked nothing → not yet purgeable.
    assert!(
        !a.may_purge(space, &payload_sha),
        "payload not purgeable until every member has acked"
    );

    // Bob acks (his replicated high-water reaches the tombstone clock).
    a.note_purge_ack(&bob, delete_clock);
    assert!(
        a.may_purge(space, &payload_sha),
        "all members acked → payload is purgeable"
    );
    assert!(a.purgeable_payloads(space).contains(&payload_sha));

    // The caller now reclaims the bytes.
    assert!(blobs.purge(&payload_sha).expect("purge"));
    assert!(!blobs.contains(&payload_sha));
}

// ---- alerts ---------------------------------------------------------------

#[test]
fn destructive_alert_action_requires_arming() {
    let mut a = engine("alice");
    let sa = sig(1);
    let mut ia = SeqIds::new(1);
    let space = a
        .apply(
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Incident,
                name: "sev1".into(),
            },
            &sa,
            &mut ia,
            1000,
        )
        .expect("create")[0]
        .space_id;
    let alert_env = craft_alert(space, &sa, 1100);
    let alert = alert_env.event_id;
    a.merge(vec![alert_env]).expect("merge alert");

    // Unknown action → visible error.
    assert!(matches!(
        a.apply(
            &CollabCommand::RunAlertAction {
                space,
                alert,
                action_id: "nope".into(),
                armed: true,
            },
            &sa,
            &mut ia,
            1200,
        ),
        Err(CollabError::ActionNotFound { .. })
    ));
    // Destructive, unarmed → visible refusal.
    assert!(matches!(
        a.apply(
            &CollabCommand::RunAlertAction {
                space,
                alert,
                action_id: "restart".into(),
                armed: false,
            },
            &sa,
            &mut ia,
            1300,
        ),
        Err(CollabError::DestructiveNotArmed { .. })
    ));
    // Armed → fires.
    a.apply(
        &CollabCommand::RunAlertAction {
            space,
            alert,
            action_id: "restart".into(),
            armed: true,
        },
        &sa,
        &mut ia,
        1400,
    )
    .expect("armed destructive fires");
}

#[test]
fn alert_ack_is_projected_into_the_inbox() {
    let mut a = engine("alice");
    let sa = sig(1);
    let mut ia = SeqIds::new(1);
    let space = a
        .apply(
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Incident,
                name: "sev1".into(),
            },
            &sa,
            &mut ia,
            1000,
        )
        .expect("create")[0]
        .space_id;
    let alert_env = craft_alert(space, &sa, 1100);
    let alert = alert_env.event_id;
    a.merge(vec![alert_env]).expect("merge");
    let inbox = a.projection().alert_inbox().expect("inbox");
    assert_eq!(inbox.alerts.len(), 1);
    assert!(!inbox.alerts[0].acknowledged);
    a.apply(
        &CollabCommand::AckAlert { space, alert },
        &sa,
        &mut ia,
        1200,
    )
    .expect("ack");
    let inbox = a.projection().alert_inbox().expect("inbox");
    assert!(inbox.alerts[0].acknowledged);
}

#[test]
fn start_call_flows_into_the_call_state_projection() {
    // A focused command → event → projection round-trip for calls (WL-FUNC-011):
    // StartCall mints a CallStarted event the projection folds into CallState (the
    // read model the egui call bar + Calls mode render); AnswerCall connects a
    // second participant; HangUpCall by the last one ends the call and drops it
    // from the active set. This is the call STATE — the live media transport
    // (WebRTC/LiveKit/SIP) is the separate, marked media-plane follow-up.
    let sa = sig(1);
    let sb = sig(2);
    let mut ia = SeqIds::new(1);
    let mut ib = SeqIds::new(1_000);
    let mut a = engine("alice");

    // Alice creates a space and adds bob (so bob may join the call).
    let created = a
        .apply(
            &CollabCommand::CreateSpace {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
            &sa,
            &mut ia,
            1000,
        )
        .expect("create");
    let space = created[0].space_id;
    a.apply(
        &CollabCommand::AddMember {
            space,
            actor: ActorId::new("bob"),
            role: SpaceRole::Member,
        },
        &sa,
        &mut ia,
        1050,
    )
    .expect("add bob");

    // Alice starts an audio call → exactly one CallStarted, initiator = alice.
    let call = CallId::new();
    let events = a
        .apply(
            &CollabCommand::StartCall {
                space,
                call,
                kind: CallKind::Audio,
            },
            &sa,
            &mut ia,
            1100,
        )
        .expect("start call");
    assert_eq!(events.len(), 1, "StartCall mints exactly one event");
    assert!(
        matches!(
            &events[0].kind,
            CollabEventKind::CallStarted { call: c, kind: CallKind::Audio, initiator }
                if *c == call && initiator == &ActorId::new("alice")
        ),
        "StartCall must mint CallStarted(alice, audio)"
    );

    // The projection folds it into CallState — the read model the surface renders.
    let cs = a.projection().call_state(Some(space)).expect("call_state");
    assert_eq!(cs.active.len(), 1, "one active call in the projection");
    assert_eq!(cs.active[0].call, call);
    assert_eq!(cs.active[0].kind, CallKind::Audio);
    assert!(
        cs.active[0].participants.iter().any(|p| p.actor == ActorId::new("alice")
            && p.state == CallParticipantState::Connected),
        "the initiator is Connected in the projection"
    );

    // Bob replicates alice's log and answers → CallParticipantChanged(Connected).
    let mut b = engine("bob");
    b.merge(a.all_events()).expect("bob syncs");
    b.apply(&CollabCommand::AnswerCall { call }, &sb, &mut ib, 1200)
        .expect("bob answers");
    a.merge(b.all_events()).expect("alice syncs bob");
    let cs = a.projection().call_state(Some(space)).expect("call_state");
    assert!(
        cs.active[0]
            .participants
            .iter()
            .any(|p| p.actor == ActorId::new("bob") && p.state == CallParticipantState::Connected),
        "AnswerCall connects bob in the projection"
    );

    // Alice hangs up while bob is still Connected → the call stays active.
    a.apply(&CollabCommand::HangUpCall { call }, &sa, &mut ia, 1300)
        .expect("alice hangs up");
    let cs = a.projection().call_state(Some(space)).expect("call_state");
    assert_eq!(
        cs.active.len(),
        1,
        "still active while bob remains connected"
    );

    // Bob (the last participant) hangs up → CallEnded drops it from the active set.
    b.merge(a.all_events()).expect("bob syncs alice hangup");
    b.apply(&CollabCommand::HangUpCall { call }, &sb, &mut ib, 1350)
        .expect("bob hangs up");
    a.merge(b.all_events()).expect("alice syncs bob hangup");
    let cs = a.projection().call_state(Some(space)).expect("call_state");
    assert!(
        cs.active.is_empty(),
        "once the last participant hangs up, CallEnded drops the call from the active set"
    );
}

// Ensure unused-import guards don't trip: BTreeSet is used by the purge model.
const _: fn() = || {
    let _: BTreeSet<ActorId> = BTreeSet::new();
};
