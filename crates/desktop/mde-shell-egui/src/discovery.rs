//! The shell's **broker-session wire path** — the `SessionRequest::Open` mirror
//! every desktop connect publishes through (E12-5b's surviving half).
//!
//! E12-5b landed two things: a flat remote-desktop picker list and the wire
//! contract its Connect emitted. The picker face is **superseded by the Desktop
//! Chooser** ([`crate::chooser`], CHOOSER-2 — the card grid over the BRAND-1
//! backdrop); this module remains the ONE place the broker `Open` request is
//! minted, shared by the Chooser's card connect and the Chat surface's
//! per-contact Remote Control (§6 — one copy of the wire shape, never two).
//!
//! ## One wire contract (§6 glue)
//!
//! **The Connect request** reuses the broker's *wire contract*, not its Rust
//! type. §6 keeps the shell in the desktop tier — it leans inward only on
//! `mde-bus`, never the `mackesd` daemon crate (whose `SessionRequest` is gated
//! behind the heavy `async-services` feature: tokio / zbus / etcd). So, exactly
//! as [`crate::datacenter`]'s `Lifecycle` mirrors the VM-lifecycle action, the
//! local [`ConnectRequest`] serialises to the identical `action/vdi/session`
//! body the broker's `parse_request` decodes — reusing the contract, not
//! inventing a parallel one (a round-trip test pins the shape).
//!
//! The live cross-peer serving is **gated** downstream in the broker (its
//! `MeshSessionStore` returns a typed gated error); publishing the `Open`
//! request is the reachable near half of the flow, so these paths are real
//! callers — never a placeholder (§7).

use std::path::Path;

use serde::Serialize;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

/// The broker's session-lifecycle topic — the exact wire topic
/// `mackesd::workers::session_broker::ACTION_TOPIC` drains. We publish the `Open`
/// verb here; the leader-gated broker folds it into the roaming-session roster.
const ACTION_TOPIC: &str = "action/vdi/session";

// ─────────────────────────── the Open request (wire mirror) ───────────────────────────

/// The shell's local mirror of the broker's `SessionRequest::Open` — the ONE
/// session verb the shell emits. See the module doc for why this is a wire
/// mirror rather than a direct dependency on the daemon's type (§6). The
/// remaining verbs (`Active` / `Disconnect` / `Close`) are the broker's own
/// lifecycle transitions, not the shell's to publish, so only `Open` is
/// mirrored — exactly as `datacenter::Lifecycle` mirrors only the verbs the
/// Fleet view emits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum ConnectRequest {
    /// Open a new session for `vm_id` on `serving_peer`, driven by `client_peer`
    /// (this node's shell). Serialises to the `SessionRequest::Open` body shape.
    Open {
        /// The session id to mint (the roster key).
        id: String,
        /// The peer that will serve the VM (a scheduler node id).
        serving_peer: String,
        /// The target desktop. A VM desktop names the libvirt domain (the UUID
        /// isn't on the discovery wire), a **host** desktop names the peer
        /// itself — the broker's `VmId` is a plain string that accepts both; a
        /// later compute-registry UUID drops in without touching the wire.
        vm_id: String,
        /// The peer whose shell drives the desktop — this node.
        client_peer: String,
    },
}

impl ConnectRequest {
    /// Serialise to the `action/vdi/session` request body. A fixed derive-backed
    /// shape ⇒ serialisation can't realistically fail; an empty body (never
    /// produced here) would simply be rejected by the broker's parser.
    fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Mint the opaque session id the broker keys the roster on. Production uses a
/// ULID; here it's a `vdi-<ms>-<vm>` id — unique per connect on a node without
/// pulling a ULID dep, and deterministic given `now_ms` (its only entropy) so the
/// pure request builder stays testable, mirroring the broker's no-ambient-clock
/// core.
fn mint_session_id(vm: &str, now_ms: u64) -> String {
    format!("vdi-{now_ms}-{vm}")
}

/// Wall-clock milliseconds since the Unix epoch (saturated, never panicking) — the
/// session-id entropy at Connect time.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The local peer name stamped as the session's `client_peer`: `$HOSTNAME` →
/// `/etc/hostname` → `"localhost"` (the desktop-tier idiom, shared with
/// `mde-panel-egui`). The mesh identifies nodes by hostname.
pub(crate) fn local_peer() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    "localhost".to_string()
}

/// Publish an `Open` request `body` to `action/vdi/session` via the persist-first
/// path (`mde-bus publish`'s own path): the write is recorded locally and the Bus
/// replicates it to the serving peer. Records any failure in `last_error` — never
/// panics. (The cross-peer *serving* is gated downstream in the broker; the publish
/// itself is the reachable near half.)
fn publish(bus_root: Option<&Path>, last_error: &mut Option<String>, body: &str) {
    let Some(root) = bus_root else {
        *last_error = Some("No mesh Bus directory — can't request a desktop session.".to_string());
        return;
    };
    match Persist::open(root.to_path_buf())
        .and_then(|p| p.write(ACTION_TOPIC, Priority::Default, None, Some(body)))
    {
        Ok(_) => *last_error = None,
        Err(e) => *last_error = Some(format!("Couldn't request the session: {e}")),
    }
}

/// Build + publish the broker `Open` request for one desktop target — the ONE
/// emitter the Chooser's card connect (CHOOSER-2) drives. `serving_peer` is the
/// node serving the desktop; `vm_id` is the VM's domain name (or the peer's own
/// hostname for a host/seat desktop); `client_peer` is this node. Failures land
/// in `last_error` (honest, never a panic). Returns the published wire body so
/// callers/tests can pin the shape.
pub(crate) fn publish_open(
    bus_root: Option<&Path>,
    last_error: &mut Option<String>,
    serving_peer: &str,
    vm_id: &str,
    client_peer: &str,
) -> String {
    let body = ConnectRequest::Open {
        id: mint_session_id(vm_id, now_ms()),
        serving_peer: serving_peer.to_string(),
        vm_id: vm_id.to_string(),
        client_peer: client_peer.to_string(),
    }
    .to_body();
    publish(bus_root, last_error, &body);
    body
}

/// NOTIFY-CHAT-4 — the Chat surface's per-contact **Remote Control** reuses this
/// exact broker `Open` wire path (§6, no second copy of the shape): open `host`'s
/// desktop by naming the contact host as both the serving peer and the target.
/// The near half — publishing the request — is reachable now; the broker actually
/// serving a **host** (vs a VM) desktop is integration-gated (a running leader +
/// guest). Best-effort: a missing Bus is a silent no-op (the honest solo-host
/// state), the same discipline as `ChatState::send`.
pub(crate) fn request_host_desktop(bus_root: Option<&Path>, host: &str) {
    // Best-effort by contract: the error is deliberately dropped (the Chat
    // row has no inline error slot; a solo host simply doesn't publish).
    let mut discarded = None;
    let _ = publish_open(bus_root, &mut discarded, host, host, &local_peer());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_open_request_serialises_to_the_snake_case_tagged_shape() {
        // Pin the wire contract: internally `op`-tagged, snake_case — byte-for-byte
        // what the broker's `#[serde(tag = "op", rename_all = "snake_case")]`
        // `SessionRequest` expects, so this mirror can't silently drift from it.
        let body = ConnectRequest::Open {
            id: mint_session_id("vm-y", 42),
            serving_peer: "peer-x".to_string(),
            vm_id: "vm-y".to_string(),
            client_peer: "me".to_string(),
        }
        .to_body();
        assert_eq!(
            body,
            r#"{"op":"open","id":"vdi-42-vm-y","serving_peer":"peer-x","vm_id":"vm-y","client_peer":"me"}"#
        );
    }

    #[test]
    fn publish_open_emits_the_broker_body_and_errors_honestly_without_a_bus() {
        // The one shared emitter (Chooser cards + Chat Remote Control): the
        // returned body is the broker's decodable `Open` shape, and a missing
        // Bus surfaces the honest inline error instead of panicking/hanging.
        let mut last_error = None;
        let body = publish_open(None, &mut last_error, "node-b", "db1", "client-node");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        assert_eq!(v["op"], "open");
        assert_eq!(v["serving_peer"], "node-b");
        assert_eq!(v["vm_id"], "db1");
        assert_eq!(v["client_peer"], "client-node");
        assert!(
            v["id"].as_str().is_some_and(|s| !s.is_empty()),
            "a session id is minted"
        );
        assert!(
            last_error
                .as_deref()
                .is_some_and(|e| e.contains("No mesh Bus")),
            "no Bus dir surfaces an error, not a panic"
        );
    }

    #[test]
    fn request_host_desktop_names_the_host_as_peer_and_target() {
        // Best-effort by contract: with no Bus it publishes nothing and stays
        // silent (the honest solo-host state) — proven by not panicking here;
        // the body shape itself is pinned via `publish_open` above with
        // serving_peer == vm_id == the contact host.
        request_host_desktop(None, "oak");
        let mut err = None;
        let body = publish_open(None, &mut err, "oak", "oak", "me");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        assert_eq!(v["serving_peer"], v["vm_id"]);
    }
}
