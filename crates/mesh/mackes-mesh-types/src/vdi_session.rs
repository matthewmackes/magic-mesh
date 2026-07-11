//! VDI session-lifecycle wire contract — the `action/vdi/session` request verb
//! shared by the `mackesd` session broker (which drains it and folds it into the
//! roaming-session roster) and the desktop shell (which mints `Open` on Connect
//! and projects the live lifecycle into the bottom rail).
//!
//! arch-2 (2026-07-11) — hoisted out of `mackesd::workers::session_broker` so the
//! two shell mirrors (`discovery`, `session_rail`) reuse the ONE type instead of
//! hand-maintaining byte-compatible copies of a wire type that can silently drift.
//! It lands here (like [`crate::mesh_storage`] / [`crate::device_control`]) so the
//! desktop tier depends only on this lightweight serde crate, never the heavy
//! `async-services`-gated (`tokio` / `zbus` / `etcd`) daemon crate (§6). `mackesd`
//! re-exports it from `session_broker` so its own `SessionRequest` paths are
//! unchanged.

use serde::{Deserialize, Serialize};

/// A session lifecycle request drained off the `action/vdi/session` topic — the
/// wire verb the shell / connect flow publishes. Internally tagged on `op`.
///
/// Field ids are plain strings on the wire: the broker's `SessionId` / `NodeId` /
/// `VmId` are all `= String` aliases, so this is byte-identical to the daemon's
/// former definition and to the shell's former `String`-typed mirrors (a variant's
/// tag plus its fields serialise in declaration order — see the wire-shape tests).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SessionRequest {
    /// Open a new session (broker state `Requested`).
    Open {
        /// The session id to mint (the roster key).
        id: String,
        /// The peer that will serve the VM (a scheduler node id).
        serving_peer: String,
        /// The target desktop: a VM desktop names the libvirt domain (the UUID
        /// isn't on the discovery wire); a **host** desktop names the peer itself.
        /// The broker's `VmId` is a plain string that accepts both.
        vm_id: String,
        /// The peer whose shell drives the desktop.
        client_peer: String,
    },
    /// The connect completed — mark the session `Active`.
    Active {
        /// The session id minted by the matching `Open`.
        id: String,
    },
    /// The link dropped — mark the session `Disconnected`.
    Disconnect {
        /// The session id minted by the matching `Open`.
        id: String,
    },
    /// The session ended — mark it `Closed` (terminal).
    Close {
        /// The session id minted by the matching `Open`.
        id: String,
    },
}

impl SessionRequest {
    /// Serialise to the `action/vdi/session` request body. A fixed derive-backed
    /// shape ⇒ serialisation can't realistically fail; an empty body (never
    /// produced here) would simply be rejected by the broker's parser.
    #[must_use]
    pub fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the exact `Open` wire bytes — the tag plus the four fields in
    /// declaration order. This is the byte-identical guarantee both the broker's
    /// `parse_request` and the shell's mirrors relied on before the fold.
    #[test]
    fn open_wire_shape_is_stable() {
        let req = SessionRequest::Open {
            id: "vdi-1-win11".into(),
            serving_peer: "anvil".into(),
            vm_id: "win11".into(),
            client_peer: "seat".into(),
        };
        assert_eq!(
            req.to_body(),
            r#"{"op":"open","id":"vdi-1-win11","serving_peer":"anvil","vm_id":"win11","client_peer":"seat"}"#
        );
    }

    /// Pins the three single-field lifecycle verbs.
    #[test]
    fn lifecycle_wire_shapes_are_stable() {
        assert_eq!(
            SessionRequest::Active { id: "s1".into() }.to_body(),
            r#"{"op":"active","id":"s1"}"#
        );
        assert_eq!(
            SessionRequest::Disconnect { id: "s1".into() }.to_body(),
            r#"{"op":"disconnect","id":"s1"}"#
        );
        assert_eq!(
            SessionRequest::Close { id: "s1".into() }.to_body(),
            r#"{"op":"close","id":"s1"}"#
        );
    }

    /// Every variant round-trips through the JSON boundary the broker parses.
    #[test]
    fn round_trips_every_variant() {
        let cases = [
            SessionRequest::Open {
                id: "s".into(),
                serving_peer: "p".into(),
                vm_id: "v".into(),
                client_peer: "c".into(),
            },
            SessionRequest::Active { id: "s".into() },
            SessionRequest::Disconnect { id: "s".into() },
            SessionRequest::Close { id: "s".into() },
        ];
        for c in cases {
            let body = c.to_body();
            let back: SessionRequest = serde_json::from_str(&body).expect("deserialize");
            assert_eq!(back, c);
        }
    }
}
