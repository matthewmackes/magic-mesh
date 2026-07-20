//! The one typed error the core returns. Every rejected command surfaces a
//! specific, human-legible variant — a denied action is *visible* (an `Err`),
//! never a silent no-op (the epic's "denied actions stay visible" rule).

use mde_collab_types::ids::{
    CallId, DocumentId, EventId, FileRefId, SpaceId, ThreadId, TransferId,
};
use mde_collab_types::ActorId;
use thiserror::Error;

/// The result alias the whole crate returns.
pub type Result<T> = core::result::Result<T, CollabError>;

/// A typed collaboration-core failure.
#[derive(Debug, Error)]
pub enum CollabError {
    // ---- Validation: existence -----------------------------------------
    /// The referenced space does not exist in the folded state.
    #[error("space {0} does not exist")]
    SpaceNotFound(SpaceId),
    /// The space exists but has been deleted (tombstoned); it accepts no
    /// further commands.
    #[error("space {0} has been deleted")]
    SpaceDeleted(SpaceId),
    /// The referenced message does not exist.
    #[error("message {0} does not exist")]
    MessageNotFound(EventId),
    /// The referenced thread does not exist.
    #[error("thread {0} does not exist")]
    ThreadNotFound(ThreadId),
    /// The referenced document does not exist.
    #[error("document {0} does not exist")]
    DocumentNotFound(DocumentId),
    /// The referenced file reference does not exist (or was unlinked).
    #[error("file reference {0} does not exist")]
    FileNotFound(FileRefId),
    /// The referenced transfer does not exist.
    #[error("transfer {0} does not exist")]
    TransferNotFound(TransferId),
    /// The referenced call does not exist.
    #[error("call {0} does not exist")]
    CallNotFound(CallId),
    /// The referenced alert does not exist.
    #[error("alert {0} does not exist")]
    AlertNotFound(EventId),
    /// The referenced clipboard item does not exist.
    #[error("clipboard item {0} does not exist")]
    ClipNotFound(EventId),

    // ---- Validation: membership + permission ---------------------------
    /// The actor is not a member of the space they addressed.
    #[error("{actor} is not a member of space {space}")]
    NotMember {
        /// The addressed space.
        space: SpaceId,
        /// The rejected actor.
        actor: ActorId,
    },
    /// The actor is already a member (a redundant add/join).
    #[error("{actor} is already a member of space {space}")]
    AlreadyMember {
        /// The space.
        space: SpaceId,
        /// The actor.
        actor: ActorId,
    },
    /// The named member is not present (a remove/role-change on a non-member).
    #[error("{actor} is not present in space {space}")]
    NotPresent {
        /// The space.
        space: SpaceId,
        /// The actor.
        actor: ActorId,
    },
    /// The action requires the `Owner` role, which the actor lacks.
    #[error("action `{action}` in space {space} requires the Owner role")]
    OwnerRequired {
        /// The space.
        space: SpaceId,
        /// The verb that was denied.
        action: &'static str,
    },
    /// Removing/demoting would leave the space with no Owner.
    #[error("action `{action}` would leave space {space} without an Owner")]
    LastOwner {
        /// The space.
        space: SpaceId,
        /// The verb that was denied.
        action: &'static str,
    },

    // ---- Validation: message edit/delete window ------------------------
    /// The actor tried to edit/delete a message they did not author.
    #[error("only the author may modify message {0}")]
    NotAuthor(EventId),
    /// The 5-minute author edit/delete window has closed.
    #[error("the {window_ms} ms edit/delete window for message {target} closed {age_ms} ms ago")]
    EditWindowExpired {
        /// The message.
        target: EventId,
        /// How old the message was when the edit/delete was attempted (ms).
        age_ms: i64,
        /// The window length (ms).
        window_ms: i64,
    },
    /// The message has already been deleted (tombstoned); no further edits.
    #[error("message {0} has been deleted")]
    TargetDeleted(EventId),

    // ---- Validation: alert actions -------------------------------------
    /// The named inline alert action does not exist on the alert.
    #[error("alert {alert} has no action `{action_id}`")]
    ActionNotFound {
        /// The alert event.
        alert: EventId,
        /// The requested action id.
        action_id: String,
    },
    /// A destructive alert action was invoked without arming it.
    #[error("destructive action `{action_id}` on alert {alert} was not armed")]
    DestructiveNotArmed {
        /// The alert event.
        alert: EventId,
        /// The action id.
        action_id: String,
    },

    // ---- I/O boundaries ------------------------------------------------
    /// A content-addressed blob's bytes did not hash to the expected digest.
    #[error("blob hash mismatch: expected {expected}, got {actual}")]
    BlobHashMismatch {
        /// The expected lower-hex SHA-256.
        expected: String,
        /// The actual lower-hex SHA-256 of the fetched bytes.
        actual: String,
    },
    /// A content-addressed blob's byte length did not match its reference.
    #[error("blob size mismatch: expected {expected} bytes, got {actual}")]
    BlobSizeMismatch {
        /// The expected length.
        expected: u64,
        /// The actual length.
        actual: u64,
    },
    /// A blob referenced by digest is not present in the store.
    #[error("blob {0} not found")]
    BlobNotFound(String),
    /// A SQLite projection failure.
    #[error("projection sql error: {0}")]
    Sql(String),
    /// A filesystem failure in the actor log or fs blob store.
    #[error("io error: {0}")]
    Io(String),
    /// A serialization/deserialization failure.
    #[error("serde error: {0}")]
    Serde(String),
}

impl From<rusqlite::Error> for CollabError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e.to_string())
    }
}

impl From<std::io::Error> for CollabError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<serde_json::Error> for CollabError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serde(e.to_string())
    }
}
