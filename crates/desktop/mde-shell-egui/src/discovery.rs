//! The shell's **broker-session wire path** — the `SessionRequest` mirror every
//! desktop connect publishes through (E12-5b's surviving half).
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
//! **The Connect request** is the shared
//! [`mackes_mesh_types::vdi_session::SessionRequest`] (arch-2, 2026-07-11) — the
//! ONE definition the broker also uses, no longer a hand-maintained mirror. §6
//! keeps the shell in the desktop tier: it leans on the lightweight shared-types
//! crate, never the `mackesd` daemon crate (whose broker is gated behind the heavy
//! `async-services` feature: tokio / zbus / etcd). So, exactly as
//! [`crate::datacenter`]'s `Lifecycle` mirrors the VM-lifecycle action, this path
//! publishes the identical `action/vdi/session` body the broker's `parse_request`
//! decodes — one type, never a parallel copy (a wire-shape test in
//! `mackes-mesh-types` pins the bytes).
//!
//! The broker's roaming-session store is live and Syncthing-backed; the shell
//! publishes `open` plus live transport lifecycle (`active` / `disconnect` /
//! `close`) through this one path so the rail and broker fold the same roster.

use std::path::Path;

use mackes_mesh_types::cloud::CLOUD_ACTION_SCHEMA_VERSION;
use mackes_mesh_types::vdi_session::{
    AppVmLaunchRequest, AppVmLifecycleState, SessionRequest,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

/// The broker's session-lifecycle topic — the exact wire topic
/// `mackesd::workers::session_broker::ACTION_TOPIC` drains. The leader-gated
/// broker folds these verbs into the roaming-session roster.
const ACTION_TOPIC: &str = "action/vdi/session";

// ───────────────────────── session lifecycle wire path ─────────────────────────
//
// The `SessionRequest` verbs are the shared
// `mackes_mesh_types::vdi_session::SessionRequest` (arch-2) — imported above, not
// mirrored here. See the module doc for the §6 layering rationale.

/// The minted `Open` body plus its session id, so callers can attach the same id
/// to subsequent live transport lifecycle messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenPublication {
    /// The broker roster key.
    pub(crate) id: String,
    /// The exact wire body published to `action/vdi/session`.
    pub(crate) body: String,
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

/// Publish one session request to `action/vdi/session` via the persist-first
/// path (`mde-bus publish`'s own path): the write is recorded locally and the Bus
/// replicates it to the serving peer. Records any failure in `last_error` — never
/// panics. The exact body is wrapped in the same schema-v1, session-targeted root
/// capability the broker verifies before folding its roster.
fn publish(bus_root: Option<&Path>, last_error: &mut Option<String>, request: &SessionRequest) {
    let Some(root) = bus_root else {
        *last_error = Some("No mesh Bus directory — can't request a desktop session.".to_string());
        return;
    };
    let body = match authorize_session_request(request) {
        Ok(body) => body,
        Err(error) => {
            *last_error = Some(format!("Couldn't authorize the desktop session: {error}"));
            return;
        }
    };
    // arch-11: writer — the shared BusReader seam is read-only; this publish keeps
    // Persist::open because it needs the write Result to set `last_error`.
    match Persist::open(root.to_path_buf())
        .and_then(|p| p.write(ACTION_TOPIC, Priority::Default, None, Some(&body)))
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
    publish_open_record(bus_root, last_error, serving_peer, vm_id, client_peer).body
}

/// Build + publish `Open`, returning both the body and minted session id.
pub(crate) fn publish_open_record(
    bus_root: Option<&Path>,
    last_error: &mut Option<String>,
    serving_peer: &str,
    vm_id: &str,
    client_peer: &str,
) -> OpenPublication {
    let id = mint_session_id(vm_id, now_ms());
    let request = SessionRequest::Open {
        id: id.clone(),
        serving_peer: serving_peer.to_string(),
        vm_id: vm_id.to_string(),
        client_peer: client_peer.to_string(),
    };
    let body = request.to_body();
    publish(bus_root, last_error, &request);
    OpenPublication { id, body }
}

/// Build + publish an App VM `OpenApp` request after validating every guest
/// identity and the explicit placement supplied by the caller.  App placement
/// is deliberately not inferred from a peer-app row: until a scheduler has
/// selected a `vm_id`, a Flatpak launch must remain unavailable rather than
/// silently falling back to a host `.desktop` execution path.
pub(crate) fn publish_app_vm_open(
    bus_root: Option<&Path>,
    last_error: &mut Option<String>,
    serving_peer: &str,
    vm_id: &str,
    client_peer: &str,
    request: AppVmLaunchRequest,
) -> Result<OpenPublication, String> {
    request.validate().map_err(|error| error.to_string())?;
    if serving_peer.trim().is_empty() || vm_id.trim().is_empty() || client_peer.trim().is_empty() {
        return Err("App VM open requires explicit serving, VM, and client peers.".to_owned());
    }
    let id = request.session_id.clone();
    let broker_request = SessionRequest::OpenApp {
        id: id.clone(),
        serving_peer: serving_peer.to_string(),
        vm_id: vm_id.to_string(),
        client_peer: client_peer.to_string(),
        app_id: request.app_id,
        catalog_revision: request.catalog_revision,
        guest_profile: request.guest_profile,
        requested_capabilities: request.requested_capabilities,
        resume: request.resume,
    };
    let body = broker_request.to_body();
    publish(bus_root, last_error, &broker_request);
    Ok(OpenPublication { id, body })
}

/// Return the broker's exact capability context for one session transition.
fn session_auth_context(request: &SessionRequest) -> (&'static str, String) {
    match request {
        SessionRequest::Open { id, .. } | SessionRequest::OpenApp { id, .. } => {
            ("vdi-session-open", format!("session:{id}"))
        }
        SessionRequest::Active { id } => ("vdi-session-active", format!("session:{id}")),
        SessionRequest::AppState { id, .. } => {
            ("vdi-session-app-state", format!("session:{id}"))
        }
        SessionRequest::Disconnect { id } => ("vdi-session-disconnect", format!("session:{id}")),
        SessionRequest::Close { id } => ("vdi-session-close", format!("session:{id}")),
    }
}

/// Add the schema marker and mint the root-only exact-body capability consumed by
/// `mackesd::workers::session_broker`. The legacy `SessionRequest` wire shape is
/// preserved inside the envelope so the shared decoder remains byte-compatible.
fn authorize_session_request(request: &SessionRequest) -> Result<String, String> {
    let mut body: serde_json::Value = serde_json::from_str(&request.to_body())
        .map_err(|error| format!("session request serialization failed: {error}"))?;
    body.as_object_mut()
        .ok_or_else(|| "session request is not a JSON object".to_string())?
        .insert(
            "schema_version".to_string(),
            serde_json::Value::from(CLOUD_ACTION_SCHEMA_VERSION),
        );
    let body = body.to_string();
    let (verb, target) = session_auth_context(request);
    crate::iac::authorize_root_mutation_body(&body, verb, "vdi-session", &target)
}

#[cfg(any(test, feature = "live-vdi"))]
fn publish_lifecycle(
    bus_root: Option<&Path>,
    last_error: &mut Option<String>,
    request: SessionRequest,
) -> String {
    let body = request.to_body();
    publish(bus_root, last_error, &request);
    body
}

/// Publish the `Active` lifecycle transition for a brokered desktop session.
#[cfg(any(test, feature = "live-vdi"))]
pub(crate) fn publish_active(
    bus_root: Option<&Path>,
    last_error: &mut Option<String>,
    id: &str,
) -> String {
    publish_lifecycle(
        bus_root,
        last_error,
        SessionRequest::Active { id: id.to_string() },
    )
}

/// Publish the `Disconnect` lifecycle transition for a brokered desktop session.
#[cfg(any(test, feature = "live-vdi"))]
pub(crate) fn publish_disconnect(
    bus_root: Option<&Path>,
    last_error: &mut Option<String>,
    id: &str,
) -> String {
    publish_lifecycle(
        bus_root,
        last_error,
        SessionRequest::Disconnect { id: id.to_string() },
    )
}

/// Publish the `Close` lifecycle transition for a brokered desktop session.
#[cfg(any(test, feature = "live-vdi"))]
pub(crate) fn publish_close(
    bus_root: Option<&Path>,
    last_error: &mut Option<String>,
    id: &str,
) -> String {
    publish_lifecycle(
        bus_root,
        last_error,
        SessionRequest::Close { id: id.to_string() },
    )
}

/// Publish a guest-owned App VM readiness update. This is intentionally a
/// separate lifecycle record: a healthy VDI transport may still be waiting for
/// placement, boot, portal startup, or application recovery.
#[cfg(any(test, feature = "live-vdi"))]
pub(crate) fn publish_app_state(
    bus_root: Option<&Path>,
    last_error: &mut Option<String>,
    id: &str,
    generation: u64,
    state: AppVmLifecycleState,
    reason: Option<String>,
) -> String {
    publish_lifecycle(
        bus_root,
        last_error,
        SessionRequest::AppState {
            id: id.to_owned(),
            generation,
            state,
            reason,
        },
    )
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
        // Pin the wire contract as this emitter produces it: internally `op`-tagged,
        // snake_case — byte-for-byte what the broker's `parse_request` decodes. Since
        // arch-2 this is the shared `SessionRequest`, so drift is structurally
        // impossible; this pins the emitter (`mint_session_id` + fields) end to end.
        let body = SessionRequest::Open {
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
    fn lifecycle_requests_serialise_to_the_broker_shapes() {
        assert_eq!(
            SessionRequest::Active {
                id: "vdi-1-web".to_string()
            }
            .to_body(),
            r#"{"op":"active","id":"vdi-1-web"}"#
        );
        assert_eq!(
            SessionRequest::Disconnect {
                id: "vdi-1-web".to_string()
            }
            .to_body(),
            r#"{"op":"disconnect","id":"vdi-1-web"}"#
        );
        assert_eq!(
            SessionRequest::Close {
                id: "vdi-1-web".to_string()
            }
            .to_body(),
            r#"{"op":"close","id":"vdi-1-web"}"#
        );
        assert_eq!(
            SessionRequest::AppState {
                id: "app-session-1".to_owned(),
                generation: 3,
                state: AppVmLifecycleState::StartingApp,
                reason: Some("portal ready".to_owned()),
            }
            .to_body(),
            r#"{"op":"app_state","id":"app-session-1","generation":3,"state":"starting_app","reason":"portal ready"}"#
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
    fn publish_open_record_returns_the_minted_session_id() {
        let mut last_error = None;
        let publication =
            publish_open_record(None, &mut last_error, "node-b", "db1", "client-node");
        let v: serde_json::Value = serde_json::from_str(&publication.body).unwrap_or_default();
        assert_eq!(Some(publication.id.as_str()), v["id"].as_str());
        assert!(
            last_error
                .as_deref()
                .is_some_and(|e| e.contains("No mesh Bus")),
            "no Bus dir surfaces an error, not a panic"
        );
    }

    #[test]
    fn publish_open_writes_a_session_targeted_capability() {
        let bus = tempfile::tempdir().unwrap();
        let mut last_error = None;
        let publication = publish_open_record(
            Some(bus.path()),
            &mut last_error,
            "node-b",
            "db1",
            "client-node",
        );
        assert!(last_error.is_none());
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let rows = persist.list_since(ACTION_TOPIC, None).unwrap();
        assert_eq!(rows.len(), 1);
        let signed: serde_json::Value =
            serde_json::from_str(rows[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(signed["schema_version"], CLOUD_ACTION_SCHEMA_VERSION);
        assert!(signed["armed_token"].as_str().is_some());
        assert_eq!(signed["id"], publication.id);
        assert_eq!(signed["op"], "open");
    }

    #[test]
    fn publish_app_vm_open_uses_explicit_placement_and_signed_lifecycle_wire() {
        let bus = tempfile::tempdir().unwrap();
        let request = AppVmLaunchRequest::new(
            "org.example.Writer",
            "catalog-7",
            "wayland-standard",
            vec!["clipboard".to_owned()],
            "app-session-7",
            true,
        )
        .unwrap();
        let mut last_error = None;
        let publication = publish_app_vm_open(
            Some(bus.path()),
            &mut last_error,
            "oak",
            "app-vm-7",
            "eagle",
            request,
        )
        .unwrap();
        assert!(last_error.is_none());
        assert_eq!(publication.id, "app-session-7");
        let rows = Persist::open(bus.path().to_path_buf())
            .unwrap()
            .list_since(ACTION_TOPIC, None)
            .unwrap();
        assert_eq!(rows.len(), 1);
        let signed: serde_json::Value =
            serde_json::from_str(rows[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(signed["op"], "open_app");
        assert_eq!(signed["vm_id"], "app-vm-7");
        assert_eq!(signed["app_id"], "org.example.Writer");
        assert_eq!(signed["armed_token"].as_str().is_some(), true);
    }

    #[test]
    fn publish_app_vm_open_rejects_missing_placement_without_bus_write() {
        let bus = tempfile::tempdir().unwrap();
        let request = AppVmLaunchRequest::new(
            "org.example.Writer",
            "catalog-7",
            "wayland-standard",
            Vec::new(),
            "app-session-7",
            false,
        )
        .unwrap();
        let mut last_error = None;
        let error = publish_app_vm_open(
            Some(bus.path()),
            &mut last_error,
            "oak",
            "",
            "eagle",
            request,
        )
        .expect_err("placement must be explicit");
        assert!(error.contains("explicit serving, VM, and client peers"));
        assert!(Persist::open(bus.path().to_path_buf())
            .unwrap()
            .list_since(ACTION_TOPIC, None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn publish_lifecycle_verbs_return_the_broker_body() {
        let mut last_error = None;
        assert_eq!(
            publish_active(None, &mut last_error, "vdi-7-oak"),
            r#"{"op":"active","id":"vdi-7-oak"}"#
        );
        assert!(
            last_error
                .as_deref()
                .is_some_and(|e| e.contains("No mesh Bus")),
            "no Bus dir surfaces an error, not a panic"
        );
        assert_eq!(
            publish_disconnect(None, &mut last_error, "vdi-7-oak"),
            r#"{"op":"disconnect","id":"vdi-7-oak"}"#
        );
        assert_eq!(
            publish_close(None, &mut last_error, "vdi-7-oak"),
            r#"{"op":"close","id":"vdi-7-oak"}"#
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
