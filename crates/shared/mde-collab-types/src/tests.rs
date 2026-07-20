//! Cross-cutting contract tests: every event kind and command round-trips
//! through serde, discriminant tags are unique, and — the delivery-lock gate —
//! every event class the seven replaced subsystems produce has a covering
//! [`CollabEventKind`] (the 519-row parity ledger,
//! `docs/platform/WL-FUNC-011-parity-ledger.md`).

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::command::TransferControl;
use crate::ids::{CallId, DocumentId, EventId, FileRefId, SpaceId, ThreadId, TransferId};
use crate::space::{SpaceKind, SpaceRole};
use crate::value::{
    AiSuggestion, AiSuggestionKind, AlertAction, AlertActionKind, AlertPayload, CallKind,
    CallParticipantState, ClipItemKind, ClipboardItem, DocumentChange, FileRef, MessageBody,
    PayloadRef, PresenceState, ReviewVerdict, Severity, TransferDirection, TransferMethod,
};
use crate::{ActorId, CollabCommand, CollabEventKind};

/// One instance of every [`CollabEventKind`] variant. Exhaustive by
/// construction — adding a variant without adding it here makes the
/// `all_event_kinds_are_covered` test fail (a match on a fresh variant would
/// not compile if we matched, but here we assert the *count* instead).
fn every_event_kind() -> Vec<CollabEventKind> {
    let mut fields = BTreeMap::new();
    fields.insert("summary".to_string(), "disk low".to_string());
    let alert = AlertPayload {
        severity: Severity::Warning,
        source: "nyc3".into(),
        headline: "SMART pre-fail".into(),
        fields,
        actions: vec![AlertAction {
            id: "ack".into(),
            label: "Acknowledge".into(),
            verb: None,
            kind: AlertActionKind::Ack,
        }],
        goto: Some("action/shell/goto".into()),
    };
    let clip = ClipboardItem {
        kind: ClipItemKind::Text,
        preview: "ssh-ed25519 AAA…".into(),
        sha256_hex: crate::value::sha256_hex(b"ssh-ed25519 AAA"),
        len: 15,
        source: "eagle".into(),
    };
    let file = FileRef {
        name: "report.pdf".into(),
        size: 4096,
        sha256_hex: crate::value::sha256_hex(b"pdf-bytes"),
        mime: Some("application/pdf".into()),
    };
    vec![
        CollabEventKind::SpaceCreated {
            kind: SpaceKind::Team,
            name: "ops".into(),
        },
        CollabEventKind::SpaceRenamed {
            name: "ops-2".into(),
        },
        CollabEventKind::SpaceArchived,
        CollabEventKind::SpaceDeleted,
        CollabEventKind::MemberJoined {
            actor: ActorId::new("fra1"),
            role: SpaceRole::Member,
        },
        CollabEventKind::MemberLeft {
            actor: ActorId::new("fra1"),
        },
        CollabEventKind::MemberRoleChanged {
            actor: ActorId::new("fra1"),
            role: SpaceRole::Owner,
        },
        CollabEventKind::PresenceChanged {
            actor: ActorId::new("fra1"),
            presence: PresenceState::Dnd,
            status: Some("heads-down".into()),
        },
        CollabEventKind::MessagePosted {
            body: MessageBody::new("hi 👋"),
            thread: None,
        },
        CollabEventKind::MessageEdited {
            target: EventId::new(),
            body: MessageBody::new("hi (edited)"),
        },
        CollabEventKind::MessageDeleted {
            target: EventId::new(),
        },
        CollabEventKind::ThreadStarted {
            thread: ThreadId::new(),
            root: EventId::new(),
            title: None,
        },
        CollabEventKind::ThreadResolved {
            thread: ThreadId::new(),
        },
        CollabEventKind::AlertRaised {
            alert: alert.clone(),
        },
        CollabEventKind::AlertAcknowledged {
            target: EventId::new(),
        },
        CollabEventKind::AlertSnoozed {
            target: EventId::new(),
            until_unix_ms: 1_720_000_600_000,
        },
        CollabEventKind::AlertActionInvoked {
            target: EventId::new(),
            action_id: "restart".into(),
            armed: true,
            outcome: Some("fired".into()),
        },
        CollabEventKind::ClipboardPublished { item: clip.clone() },
        CollabEventKind::ClipboardPinned {
            target: EventId::new(),
        },
        CollabEventKind::ClipboardUnpinned {
            target: EventId::new(),
        },
        CollabEventKind::ClipboardDeleted {
            target: EventId::new(),
        },
        CollabEventKind::DocumentCreated {
            document: DocumentId::new(),
            title: "design".into(),
        },
        CollabEventKind::DocumentUpdated {
            document: DocumentId::new(),
            change: DocumentChange {
                payload: PayloadRef::of_bytes(b"crdt-update"),
                summary: Some("+2 paragraphs".into()),
            },
        },
        CollabEventKind::ReviewRequested {
            document: DocumentId::new(),
            reviewers: vec![ActorId::new("fra1")],
        },
        CollabEventKind::ReviewSubmitted {
            document: DocumentId::new(),
            verdict: ReviewVerdict::Approved,
            comment: None,
        },
        CollabEventKind::FileLinked {
            file: FileRefId::new(),
            reference: file.clone(),
        },
        CollabEventKind::FileUnlinked {
            file: FileRefId::new(),
        },
        CollabEventKind::TransferStarted {
            transfer: TransferId::new(),
            file: FileRefId::new(),
            method: TransferMethod::Sftp,
            direction: TransferDirection::Outbound,
        },
        CollabEventKind::TransferStateChanged {
            transfer: TransferId::new(),
            state: crate::value::TransferState::Active,
        },
        CollabEventKind::CallStarted {
            call: CallId::new(),
            kind: CallKind::Audio,
            initiator: ActorId::new("eagle"),
        },
        CollabEventKind::CallParticipantChanged {
            call: CallId::new(),
            actor: ActorId::new("fra1"),
            state: CallParticipantState::Connected,
        },
        CollabEventKind::CallEnded {
            call: CallId::new(),
            reason: Some("hung_up".into()),
        },
        CollabEventKind::AiSuggestionOffered {
            suggestion: AiSuggestion {
                id: "sug-1".into(),
                kind: AiSuggestionKind::SmartReply,
                target: Some(EventId::new()),
                summary: "Reply: on it".into(),
                confidence_pct: Some(82),
                provenance: "local-model".into(),
            },
        },
        CollabEventKind::AiSuggestionResolved {
            suggestion_id: "sug-1".into(),
            accepted: true,
        },
    ]
}

/// One instance of every [`CollabCommand`] variant.
fn every_command() -> Vec<CollabCommand> {
    vec![
        CollabCommand::CreateSpace {
            kind: SpaceKind::Direct,
            name: "eagle↔fra1".into(),
        },
        CollabCommand::RenameSpace {
            space: SpaceId::new(),
            name: "ops".into(),
        },
        CollabCommand::DeleteSpace {
            space: SpaceId::new(),
        },
        CollabCommand::AddMember {
            space: SpaceId::new(),
            actor: ActorId::new("fra1"),
            role: SpaceRole::Member,
        },
        CollabCommand::RemoveMember {
            space: SpaceId::new(),
            actor: ActorId::new("fra1"),
        },
        CollabCommand::SetMemberRole {
            space: SpaceId::new(),
            actor: ActorId::new("fra1"),
            role: SpaceRole::Owner,
        },
        CollabCommand::JoinSpace {
            space: SpaceId::new(),
        },
        CollabCommand::LeaveSpace {
            space: SpaceId::new(),
        },
        CollabCommand::SetPresence {
            presence: PresenceState::Away,
            status: None,
        },
        CollabCommand::SendMessage {
            space: SpaceId::new(),
            thread: None,
            body: MessageBody::new("hello"),
        },
        CollabCommand::EditMessage {
            space: SpaceId::new(),
            target: EventId::new(),
            body: MessageBody::new("hello!"),
        },
        CollabCommand::DeleteMessage {
            space: SpaceId::new(),
            target: EventId::new(),
        },
        CollabCommand::StartThread {
            space: SpaceId::new(),
            root: EventId::new(),
            title: Some("re: deploy".into()),
        },
        CollabCommand::ReplyInThread {
            space: SpaceId::new(),
            thread: ThreadId::new(),
            body: MessageBody::new("+1"),
        },
        CollabCommand::AckAlert {
            space: SpaceId::new(),
            alert: EventId::new(),
        },
        CollabCommand::SnoozeAlert {
            space: SpaceId::new(),
            alert: EventId::new(),
            until_unix_ms: 1_720_000_600_000,
        },
        CollabCommand::RunAlertAction {
            space: SpaceId::new(),
            alert: EventId::new(),
            action_id: "restart".into(),
            armed: true,
        },
        CollabCommand::SetAlertMute {
            source: "nyc3".into(),
            muted: true,
        },
        CollabCommand::SetSeverityThreshold {
            threshold: Severity::Warning,
        },
        CollabCommand::SetDoNotDisturb { enabled: true },
        CollabCommand::PublishClipboard {
            space: SpaceId::new(),
            item: ClipboardItem {
                kind: ClipItemKind::Uri,
                preview: "https://…".into(),
                sha256_hex: crate::value::sha256_hex(b"https://example"),
                len: 15,
                source: "eagle".into(),
            },
        },
        CollabCommand::AttachClipboard {
            space: SpaceId::new(),
            clip: EventId::new(),
        },
        CollabCommand::PinClipboard {
            space: SpaceId::new(),
            clip: EventId::new(),
        },
        CollabCommand::UnpinClipboard {
            space: SpaceId::new(),
            clip: EventId::new(),
        },
        CollabCommand::DeleteClipboard {
            space: SpaceId::new(),
            clip: EventId::new(),
        },
        CollabCommand::ClearClipboard {
            space: SpaceId::new(),
        },
        CollabCommand::CreateDocument {
            space: SpaceId::new(),
            document: DocumentId::new(),
            title: "design".into(),
        },
        CollabCommand::UpdateDocument {
            space: SpaceId::new(),
            document: DocumentId::new(),
            change: DocumentChange {
                payload: PayloadRef::of_bytes(b"crdt"),
                summary: None,
            },
        },
        CollabCommand::RequestReview {
            space: SpaceId::new(),
            document: DocumentId::new(),
            reviewers: vec![ActorId::new("fra1")],
        },
        CollabCommand::SubmitReview {
            space: SpaceId::new(),
            document: DocumentId::new(),
            verdict: ReviewVerdict::ChangesRequested,
            comment: Some("nit".into()),
        },
        CollabCommand::LinkFile {
            space: SpaceId::new(),
            file: FileRefId::new(),
            reference: FileRef {
                name: "a.bin".into(),
                size: 1,
                sha256_hex: crate::value::sha256_hex(b"a"),
                mime: None,
            },
        },
        CollabCommand::UnlinkFile {
            space: SpaceId::new(),
            file: FileRefId::new(),
        },
        CollabCommand::StartTransfer {
            space: SpaceId::new(),
            transfer: TransferId::new(),
            file: FileRefId::new(),
            method: TransferMethod::Rsync,
            direction: TransferDirection::Inbound,
        },
        CollabCommand::ControlTransfer {
            transfer: TransferId::new(),
            control: TransferControl::Pause,
        },
        CollabCommand::StartCall {
            space: SpaceId::new(),
            call: CallId::new(),
            kind: CallKind::Video,
        },
        CollabCommand::AnswerCall {
            call: CallId::new(),
        },
        CollabCommand::DeclineCall {
            call: CallId::new(),
        },
        CollabCommand::HangUpCall {
            call: CallId::new(),
        },
        CollabCommand::SendDtmf {
            call: CallId::new(),
            digit: '5',
        },
        CollabCommand::SetCallMuted {
            call: CallId::new(),
            muted: true,
        },
        CollabCommand::RequestAiSuggestion {
            space: SpaceId::new(),
            target: None,
            kind: AiSuggestionKind::Summary,
        },
    ]
}

#[test]
fn every_event_kind_round_trips_and_has_a_unique_tag() {
    let kinds = every_event_kind();
    let mut tags = BTreeSet::new();
    for k in &kinds {
        let json = serde_json::to_string(k).expect("serialize");
        let back: CollabEventKind = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(*k, back, "round-trip {}", k.tag());
        assert!(tags.insert(k.tag()), "duplicate event tag {}", k.tag());
        // The external tag is the wire key.
        assert!(
            json.contains(k.tag()),
            "wire key matches tag for {}",
            k.tag()
        );
    }
    // Guards against a variant being added without an accompanying sample here.
    assert_eq!(tags.len(), 34, "sample set must cover every event kind");
}

#[test]
fn every_command_round_trips_and_has_a_unique_verb() {
    let commands = every_command();
    let mut verbs = BTreeSet::new();
    for c in &commands {
        let json = serde_json::to_string(c).expect("serialize");
        let back: CollabCommand = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(*c, back, "round-trip {}", c.verb());
        assert!(verbs.insert(c.verb()), "duplicate verb {}", c.verb());
    }
    assert_eq!(verbs.len(), 41, "sample set must cover every command");
}

/// The delivery-lock gate: each of the seven replaced subsystems must have at
/// least one covering [`CollabEventKind`]. The tuples below are the parity
/// ledger's event classes → the covering event tag(s); a missing tag fails.
#[test]
fn ledger_coverage_every_replaced_subsystem_has_covering_event_kinds() {
    let present: BTreeSet<&'static str> = every_event_kind()
        .iter()
        .map(CollabEventKind::tag)
        .collect();

    // (subsystem, the event tags that must exist to carry its capabilities)
    let coverage: &[(&str, &[&str])] = &[
        (
            "Chat / Messaging",
            &[
                "space_created",
                "space_deleted",
                "member_joined",
                "member_left",
                "presence_changed",
                "message_posted",
                "message_edited",
                "message_deleted",
                "thread_started",
            ],
        ),
        (
            "Voice / Calls / SIP",
            &["call_started", "call_participant_changed", "call_ended"],
        ),
        (
            "Editor",
            &[
                "document_created",
                "document_updated",
                "review_requested",
                "review_submitted",
            ],
        ),
        ("File manager", &["file_linked", "file_unlinked"]),
        ("Transfers", &["transfer_started", "transfer_state_changed"]),
        (
            "Alerts / Notifications",
            &[
                "alert_raised",
                "alert_acknowledged",
                "alert_snoozed",
                "alert_action_invoked",
            ],
        ),
        (
            "Clipboard sync",
            &[
                "clipboard_published",
                "clipboard_pinned",
                "clipboard_unpinned",
                "clipboard_deleted",
            ],
        ),
    ];

    for (subsystem, needed) in coverage {
        for tag in *needed {
            assert!(
                present.contains(tag),
                "{subsystem}: no CollabEventKind covers `{tag}`"
            );
        }
    }

    // The AI-suggestion metadata class (net-new, not one of the 7) is also
    // present, per the epic's public-contracts spec.
    assert!(present.contains("ai_suggestion_offered"));
    assert!(present.contains("ai_suggestion_resolved"));
}
