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

use std::path::{Path, PathBuf};

use mackes_mesh_types::cloud::{
    cloud_request_digest, CloudArmSigner, CloudArmedToken, CLOUD_ACTION_SCHEMA_VERSION,
};
#[cfg(not(test))]
use mackes_mesh_types::cloud::{decode_cloud_arm_credential, CLOUD_ARM_CREDENTIAL};
use serde::Serialize;

use mde_chat::MessageKind;

/// The `action/chat/send` verb the mackesd `chat` worker drains.
///
/// (Its `ACTION_CHAT_SEND`.) A JSON boundary — this surface owns a local mirror
/// of the worker's request shape, never a dep on `mackesd`.
pub const ACTION_CHAT_SEND: &str = "action/chat/send";

/// The exact capability verb for a local chat-send mutation.
const CHAT_AUTH_VERB: &str = "chat-send";

/// Keep a chat capability useful for one Bus drain while limiting replay value.
const ACTION_TOKEN_TTL_MS: i64 = 30_000;

/// A local mirror of the worker's `action/chat/send` request body (its private
/// `SendRequest`): a 1:1 `peer` scope, the recipient contact (the hostname *is*
/// the username, lock 2/21), and a typed [`MessageKind`] `kind` — a `kind` wins
/// over `text` in the worker, so this posts a real File card.
#[derive(Serialize)]
struct ChatSend<'a> {
    /// Schema version required by the privileged action contract.
    schema_version: u16,
    /// `"peer"` — a 1:1 conversation (the worker's `Scope::Peer`, `snake_case`).
    scope: &'a str,
    /// The recipient contact: the peer **host** (username = hostname).
    to: &'a str,
    /// The typed message body — a [`MessageKind::File`] offer.
    kind: MessageKind,
}

/// Load the production mint authority from the root DRM shell's sealed systemd
/// credential. Test builds use the existing deterministic injection seam so the
/// Bus contract remains executable without a live service credential.
fn action_signer() -> Result<CloudArmSigner, String> {
    #[cfg(test)]
    {
        return CloudArmSigner::new(b"0123456789abcdef0123456789abcdef".to_vec())
            .map_err(str::to_string);
    }
    #[cfg(not(test))]
    {
        if !rustix::process::geteuid().is_root() {
            return Err(
                "Chat-send authorization is available only in the root DRM shell.".to_string(),
            );
        }
        let directory = std::env::var_os("CREDENTIALS_DIRECTORY")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| {
                "The root shell has no systemd action credential; chat sends are disabled."
                    .to_string()
            })?;
        let path = directory.join(CLOUD_ARM_CREDENTIAL);
        let raw = std::fs::read(&path)
            .map_err(|e| format!("Could not read systemd action credential: {e}"))?;
        let key = decode_cloud_arm_credential(&raw).map_err(str::to_string)?;
        CloudArmSigner::new(key).map_err(str::to_string)
    }
}

/// Strip the daemon's `peer:` transport prefix from a configured node id.
///
/// `mackesd` passes this same bare value to the Chat worker as the actor
/// identity. Keeping the normalization here (rather than trusting the seat's
/// `HOSTNAME`) prevents a configured node id and the capability placement from
/// describing different actors.
fn bare_node_id(node_id: &str) -> Option<&str> {
    let node = node_id.strip_prefix("peer:").unwrap_or(node_id);
    (!node.is_empty()).then_some(node)
}

/// Resolve this node's mesh identity for the capability's placement field.
/// `MACKESD_NODE_ID` is authoritative because it is what the daemon uses to
/// construct the Chat worker actor; the hostname sources are only fallbacks.
fn local_node() -> String {
    if let Ok(node_id) = std::env::var("MACKESD_NODE_ID") {
        if let Some(node) = bare_node_id(&node_id) {
            return node.to_string();
        }
    }
    if let Ok(hostname) = std::env::var("HOSTNAME") {
        let hostname = hostname.trim();
        if !hostname.is_empty() {
            return hostname.to_string();
        }
    }
    for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(hostname) = std::fs::read_to_string(path) {
            let hostname = hostname.trim();
            if !hostname.is_empty() {
                return hostname.to_string();
            }
        }
    }
    "node".to_string()
}

/// Validate the `peer` addressee before it becomes part of a conversation key.
///
/// The Chat wire calls this field a hostname, not an arbitrary room/key string.
/// Restricting it to DNS-style labels also prevents `/`, `\\`, control
/// characters, and `room:`-shaped values from crossing the peer/room boundary
/// or becoming path components in the worker's durable per-conversation log.
fn validate_peer_host(to: &str) -> Result<(), String> {
    if to.is_empty() || to.len() > 251 || to.trim() != to {
        return Err("Chat-send target must be a bare peer hostname.".to_string());
    }
    for label in to.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("Chat-send target must be a bare peer hostname.".to_string());
        }
    }
    Ok(())
}

/// Add schema 1 and an exact-body, short-lived HMAC capability to one chat
/// request. The unsigned JSON, including schema 1, is what the token hashes.
fn authorize_chat_body(to: &str, body: &str) -> Result<String, String> {
    validate_peer_host(to)?;
    let signer = action_signer()?;
    let mut document: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Invalid chat-send request body: {e}"))?;
    let object = document
        .as_object_mut()
        .ok_or_else(|| "Chat-send request body is not a JSON object.".to_string())?;
    object.remove("armed_token");
    object.insert(
        "schema_version".to_string(),
        serde_json::Value::from(CLOUD_ACTION_SCHEMA_VERSION),
    );
    let node = local_node();
    let target = format!("peer:{to}");
    for (label, value) in [
        ("verb", CHAT_AUTH_VERB),
        ("node", node.as_str()),
        ("target", target.as_str()),
    ] {
        if value.contains('|') || value.len() > 255 || value.trim().is_empty() {
            return Err(format!(
                "Chat-send authorization {label} is not capability-safe."
            ));
        }
    }
    let unsigned = document.to_string();
    use rand::RngCore as _;
    let mut nonce_bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = nonce_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .map_err(|_| "The system clock is before the Unix epoch.".to_string())?;
    let token = CloudArmedToken::mint(
        &signer,
        &nonce,
        now.saturating_add(ACTION_TOKEN_TTL_MS),
        CHAT_AUTH_VERB,
        &node,
        &target,
        &cloud_request_digest(&unsigned).map_err(str::to_string)?,
    )
    .encode();
    document
        .as_object_mut()
        .expect("validated object")
        .insert("armed_token".to_string(), serde_json::Value::String(token));
    serde_json::to_string(&document).map_err(|e| format!("Couldn't encode chat send: {e}"))
}

/// Build the `action/chat/send` body offering a file to `to`'s conversation.
///
/// A [`MessageKind::File`] carrying `name` + `size_bytes`; the unit-tested
/// request builder used by [`BusChatBridge::offer_file`]. The returned body carries schema 1 and
/// a short-lived capability bound to the exact unsigned request.
///
/// # Errors
/// Returns an encoding, credential, clock, or capability-scope error; production
/// callers therefore fail closed when the root systemd credential is unavailable.
#[must_use]
pub fn chat_file_offer_body(to: &str, name: &str, size_bytes: u64) -> Result<String, String> {
    let send = ChatSend {
        schema_version: CLOUD_ACTION_SCHEMA_VERSION,
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
    let body = serde_json::to_string(&send)
        .map_err(|e| format!("Couldn't encode the file chat offer: {e}"))?;
    authorize_chat_body(to, &body)
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
    bus_root: Option<PathBuf>,
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
        let Ok(body) = chat_file_offer_body(to, &name, size_bytes) else {
            return; // missing root credential or unsafe target = fail closed
        };
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
        let body = chat_file_offer_body("nyc3", "report.pdf", 4096).expect("encode");
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["schema_version"], CLOUD_ACTION_SCHEMA_VERSION);
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
        let body = chat_file_offer_body("eagle", "iso.img", 999).expect("encode");
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
    fn offer_body_has_an_exact_body_bound_short_lived_capability() {
        let body = chat_file_offer_body("eagle", "iso.img", 999).expect("encode");
        let mut value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let token = CloudArmedToken::parse(value["armed_token"].as_str().unwrap()).unwrap();
        assert_eq!(token.verb, CHAT_AUTH_VERB);
        assert_eq!(token.node, local_node());
        assert_eq!(token.target, "peer:eagle");
        assert!(token.expires_at_ms > 0);
        value.as_object_mut().unwrap().remove("armed_token");
        assert_eq!(
            token.request_sha256,
            cloud_request_digest(&value.to_string()).unwrap()
        );
        let signer = action_signer().unwrap();
        assert!(signer.verify_payload(&token.signing_payload(), &token.signature));
    }

    #[test]
    fn offer_body_rejects_an_unsafe_capability_target() {
        for target in [
            "eagle|forged",
            "eagle/../../outside",
            "room:ops",
            "eagle\\\"oak",
            "eagle oak",
            "",
        ] {
            assert!(
                chat_file_offer_body(target, "iso.img", 999).is_err(),
                "malformed peer target was accepted: {target:?}"
            );
        }
    }

    #[test]
    fn configured_peer_node_id_is_normalized_to_the_worker_actor() {
        assert_eq!(bare_node_id("peer:eagle"), Some("eagle"));
        assert_eq!(bare_node_id("eagle"), Some("eagle"));
        assert_eq!(bare_node_id("peer:"), None);
    }

    #[test]
    fn no_bus_root_is_a_silent_no_op() {
        // The honest solo-host path: no Bus dir → offer_file does nothing, no panic.
        let bridge = BusChatBridge::with_root(None);
        bridge.offer_file("nyc3", Path::new("/tmp/whatever.txt"));
    }
}
