//! `ChatBridge` — the NOTIFY-CHAT hand-off seam (FILEMGR-12 "Send in Chat").
//!
//! Reuse, not reimplementation (§6): a file offered to a peer's conversation is
//! handed over as the **existing** `mde-chat` file message-kind
//! ([`MessageKind::File`]), published on the **existing** `action/chat/send`
//! verb the mackesd `chat` worker already drains — the same wire
//! `mde-shell-egui::chat::send_file` uses, but carrying a real typed `kind`
//! (which the worker folds into a rich File card) rather than the shell's older
//! text fallback. The bytes still move over the FILEMGR-7 Send-To path (the
//! model does that half); this seam only posts the *offer* into the timeline.
//!
//! Injectable like [`MeshMountClient`](crate::mesh_mount::MeshMountClient): the
//! production [`BusChatBridge`] opens a local `Persist` and writes the verb (the
//! same persist-first path `BusMeshMount` takes); a test injects a fake and
//! asserts the exact offer. A missing Bus is a silent no-op — the honest
//! solo-host state — never a panic and never a hang.

use std::path::Path;

use serde::Serialize;

use mde_chat::MessageKind;

/// The `action/chat/send` verb the mackesd `chat` worker drains.
///
/// (Its `ACTION_CHAT_SEND`.) A JSON boundary — this surface owns a local mirror
/// of the worker's request shape, never a dep on `mackesd`.
pub const ACTION_CHAT_SEND: &str = "action/chat/send";

/// A local mirror of the worker's `action/chat/send` request body (its private
/// `SendRequest`): a 1:1 `peer` scope, the recipient contact (the hostname *is*
/// the username, lock 2/21), and a typed [`MessageKind`] `kind` — a `kind` wins
/// over `text` in the worker, so this posts a real File card.
#[derive(Serialize)]
struct ChatSend<'a> {
    /// `"peer"` — a 1:1 conversation (the worker's `Scope::Peer`, `snake_case`).
    scope: &'a str,
    /// The recipient contact: the peer **host** (username = hostname).
    to: &'a str,
    /// The typed message body — a [`MessageKind::File`] offer.
    kind: MessageKind,
}

/// Build the `action/chat/send` body offering a file to `to`'s conversation.
///
/// A [`MessageKind::File`] carrying `name` + `size_bytes`; the pure, unit-tested
/// core of [`BusChatBridge::offer_file`] — no I/O, so the exact wire shape is
/// asserted headless (it round-trips back into a `file`-kind the worker accepts).
#[must_use]
pub fn chat_file_offer_body(to: &str, name: &str, size_bytes: u64) -> String {
    let send = ChatSend {
        scope: "peer",
        to,
        // `mime` stays `None` — the file kind's MIME is "when the sender knew
        // it" (message.rs), and Files doesn't sniff one here; honest over faked.
        kind: MessageKind::File {
            name: name.to_string(),
            size_bytes,
            mime: None,
        },
    };
    serde_json::to_string(&send).unwrap_or_default()
}

/// The "hand a file to a chat conversation" seam. Production is
/// [`BusChatBridge`]; tests inject a recorder.
pub trait ChatBridge {
    /// Offer `path` to the `to` contact's conversation as a File message-kind.
    /// Best-effort — a missing Bus / open failure is a silent no-op, never a
    /// panic. `to` is the peer **host** (the chat contact username).
    fn offer_file(&self, to: &str, path: &Path);
}

/// The live Bus-backed bridge — a synchronous local `Persist` write onto
/// `action/chat/send`.
///
/// The same persist-first path [`BusMeshMount`](crate::mesh_mount::BusMeshMount)
/// uses. Holds only the resolved Bus spool dir; a fresh `Persist` opens per call
/// (it isn't `Send`).
pub struct BusChatBridge {
    /// The resolved Bus client spool dir, or `None` when this node has no Bus.
    bus_root: Option<std::path::PathBuf>,
}

impl BusChatBridge {
    /// Resolve the Bus spool dir from the environment (the production path).
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
        }
    }

    /// Construct with an explicit spool root (tests point this at a tempdir, or
    /// `None` to exercise the honest no-Bus no-op).
    #[must_use]
    pub fn with_root(bus_root: Option<std::path::PathBuf>) -> Self {
        Self { bus_root }
    }
}

impl ChatBridge for BusChatBridge {
    fn offer_file(&self, to: &str, path: &Path) {
        let Some(root) = self.bus_root.clone() else {
            return; // no Bus on this node — the honest solo-host no-op
        };
        let Ok(persist) = mde_bus::persist::Persist::open(root) else {
            return; // a transient open failure = a silent no-op, never a panic
        };
        let name = path.file_name().map_or_else(
            || path.to_string_lossy().into_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        // A real metadata read (best-effort): 0 when the file is gone, never faked.
        let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let body = chat_file_offer_body(to, &name, size_bytes);
        let _ = persist.write(
            ACTION_CHAT_SEND,
            mde_bus::hooks::config::Priority::Default,
            None,
            Some(&body),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_body_is_a_peer_scoped_file_kind() {
        let body = chat_file_offer_body("nyc3", "report.pdf", 4096);
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["scope"], "peer");
        assert_eq!(v["to"], "nyc3");
        // The worker reads `kind` as an mde-chat MessageKind, snake_case-tagged.
        assert_eq!(v["kind"]["file"]["name"], "report.pdf");
        assert_eq!(v["kind"]["file"]["size_bytes"], 4096);
        assert!(v["kind"]["file"]["mime"].is_null());
    }

    #[test]
    fn offer_body_round_trips_into_a_file_message_kind() {
        // Prove it's the REAL mde-chat file kind (not a hand-rolled shape): the
        // `kind` object deserializes straight back into MessageKind::File.
        let body = chat_file_offer_body("eagle", "iso.img", 999);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let kind: MessageKind = serde_json::from_value(v["kind"].clone()).expect("a MessageKind");
        assert_eq!(kind.tag(), "file");
        assert!(matches!(
            kind,
            MessageKind::File {
                size_bytes: 999,
                ..
            }
        ));
    }

    #[test]
    fn no_bus_root_is_a_silent_no_op() {
        // The honest solo-host path: no Bus dir → offer_file does nothing, no panic.
        let bridge = BusChatBridge::with_root(None);
        bridge.offer_file("nyc3", Path::new("/tmp/whatever.txt"));
    }
}
