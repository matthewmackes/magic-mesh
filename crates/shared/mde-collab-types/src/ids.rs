//! The stable, opaque identifiers every Communications contract is keyed by.
//!
//! Each id is a newtype over a [`Uuid`]. It is **opaque** (a reader learns
//! nothing from its bytes) and **stable**: minted once, it survives the object
//! moving between paths/spaces, a node reconnecting, an event log being
//! replayed, and the same object being linked into multiple spaces. Nothing in
//! an id is derived from a filesystem path, a hostname, or an ordinal, so none
//! of those changing perturbs it.
//!
//! The seven identifier types the epic locks:
//!
//! * [`SpaceId`] — a conversation/team/incident/project space.
//! * [`EventId`] — one signed [`CollabEventEnvelope`](crate::CollabEventEnvelope)
//!   in a space's log (a message, alert, membership change, …).
//! * [`ThreadId`] — a reply thread rooted at some event.
//! * [`DocumentId`] — a collaboratively-edited document.
//! * [`FileRefId`] — a file reference linked into a space.
//! * [`TransferId`] — a file-transfer job (the control handle; the byte-level
//!   progress ledger is WL-FUNC-006's, referenced by this id, never duplicated).
//! * [`CallId`] — a call / co-edit / remote-desktop session.

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Define one opaque UUID-newtype identifier with the full contract surface:
/// `Copy`/`Eq`/`Hash`/`Ord`/serde (transparent — the wire form is the bare
/// UUID string), a random `new()`, a `from_uuid`/`as_uuid`, a `nil()` sentinel,
/// `Display`, and `FromStr` (so a topic segment parses back to an id).
macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Mint a fresh, random (UUIDv4) id. Two calls never collide.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wrap an existing [`Uuid`] verbatim (e.g. a deterministic id in a
            /// test, or an id already minted upstream).
            #[must_use]
            pub const fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            /// The inner [`Uuid`].
            #[must_use]
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }

            /// The all-zero sentinel id — a stable, comparable "none" that is
            /// never handed out by [`new`](Self::new).
            #[must_use]
            pub const fn nil() -> Self {
                Self(Uuid::nil())
            }

            /// Whether this is the [`nil`](Self::nil) sentinel.
            #[must_use]
            pub const fn is_nil(&self) -> bool {
                self.0.is_nil()
            }
        }

        // A random `new()` needs a matching `Default` (both to satisfy the
        // `new_without_default` lint and because an id is meaningfully
        // default-constructible: a fresh unique value).
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl From<Uuid> for $name {
            fn from(id: Uuid) -> Self {
                Self(id)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(s)?))
            }
        }
    };
}

define_id! {
    /// A Communications space: a Direct/Team/Incident/Project container that
    /// owns a signed event log, membership, and every object referenced into it.
    SpaceId
}

define_id! {
    /// One signed event in a space's log. Every
    /// [`CollabEventEnvelope`](crate::CollabEventEnvelope) carries exactly one,
    /// and edits/acks/deletes reference it by this id.
    EventId
}

define_id! {
    /// A reply thread inside a space, rooted at a message [`EventId`].
    ThreadId
}

define_id! {
    /// A collaboratively-edited document (the Documents mode / co-edit session).
    DocumentId
}

define_id! {
    /// A file linked into a space. Stable across the file moving on disk or
    /// being re-shared, so a reference never dangles when the path changes.
    FileRefId
}

define_id! {
    /// A file-transfer job. This is the control handle only — the authoritative
    /// byte-progress ledger belongs to WL-FUNC-006 and is keyed by this same id.
    TransferId
}

define_id! {
    /// A call / co-edit / remote-desktop session, from ringing through teardown.
    CallId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_opaque() {
        let a = SpaceId::new();
        let b = SpaceId::new();
        assert_ne!(a, b, "fresh ids never collide");
        // Opaque: the string form is a bare UUID, nothing structural leaks.
        assert_eq!(a.to_string().len(), 36, "canonical hyphenated UUID");
        assert!(!a.is_nil());
        assert!(SpaceId::nil().is_nil());
    }

    #[test]
    fn round_trips_through_uuid_and_string() {
        let id = EventId::new();
        assert_eq!(EventId::from_uuid(id.as_uuid()), id);
        let parsed: EventId = id.to_string().parse().expect("parse own Display");
        assert_eq!(parsed, id, "Display <-> FromStr is stable");
    }

    #[test]
    fn serde_is_transparent_uuid_string() {
        let id = DocumentId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        // Transparent: serializes as the quoted UUID string, not a wrapper obj.
        assert_eq!(json, format!("\"{id}\""));
        let back: DocumentId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, id);
    }

    #[test]
    fn distinct_id_types_do_not_alias_each_other() {
        // Same underlying UUID, but the types are distinct — a compile-time
        // guarantee a SpaceId can never be passed where a CallId is wanted.
        let u = Uuid::new_v4();
        let s = SpaceId::from_uuid(u);
        let c = CallId::from_uuid(u);
        assert_eq!(s.as_uuid(), c.as_uuid());
    }

    #[test]
    fn ids_are_orderable_for_deterministic_merge() {
        let mut v = [
            TransferId::from_uuid(Uuid::from_u128(3)),
            TransferId::from_uuid(Uuid::from_u128(1)),
            TransferId::from_uuid(Uuid::from_u128(2)),
        ];
        v.sort();
        assert_eq!(v[0].as_uuid(), Uuid::from_u128(1));
        assert_eq!(v[2].as_uuid(), Uuid::from_u128(3));
    }
}
