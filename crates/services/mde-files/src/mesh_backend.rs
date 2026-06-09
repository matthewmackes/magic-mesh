//! Mesh-first backend — reads mackesd's live Nebula status over
//! the mesh **Bus** (`action/nebula/{status,self-node,list-peers}`)
//! and exposes a mesh-flavoured API the UI binds against (peer
//! roster, overlay status).
//!
//! Why this exists. The `DBusBackend` companion talks to
//! `dev.mackes.MDE.Fleet.Files`, which returns `[]` for
//! `ListPeer(_)` because mackesd doesn't maintain a per-peer file
//! index. For a mesh-first manager the right source of truth is
//! the Nebula status surface — peer reachability + overlay IPs +
//! handshake age. Live as of NF-Bundle-0 (v2.5).
//!
//! E0.3.1.a (2026-06-03): migrated off the `dev.mackes.MDE.Nebula.
//! Status` D-Bus reads onto mackesd's Bus responder — the same
//! request/reply round-trip the wizard preview page uses. No new
//! surface is added; the reply bodies are byte-identical to the
//! old D-Bus replies, so the pure parsers below are unchanged.

#![cfg(feature = "dbus")]

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use tokio::runtime::Runtime;

/// Errors a mesh-backend call can surface. `Unavailable` is the
/// common case (mackesd not running); `Decode` only fires when
/// the daemon's JSON shape drifts past what this client parses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshError {
    /// mackesd isn't on the session bus, or the call timed out
    /// before a reply arrived.
    Unavailable(String),
    /// Daemon replied but the JSON didn't deserialize.
    Decode(String),
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(s) => write!(f, "mesh unavailable: {s}"),
            Self::Decode(s) => write!(f, "mesh decode failed: {s}"),
        }
    }
}

impl std::error::Error for MeshError {}

// ----- wire types (mirror mackesd's IPC structs) ----------------

/// Nebula `Status()` snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NebulaStatus {
    pub is_lighthouse: bool,
    pub ca_epoch: i64,
    pub peer_count: usize,
    pub mesh_id: String,
    pub active_transport: String,
}

/// Nebula `ListPeers()` row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NebulaPeer {
    pub node_id: String,
    pub name: String,
    pub overlay_ip: String,
    pub fingerprint: String,
    pub cert_epoch: i64,
    pub cert_expires_at: i64,
    /// "online" / "idle" / "offline"
    pub reachable: String,
}

/// Nebula `SelfNode()` snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NebulaSelfNode {
    pub node_id: String,
    pub host: String,
    pub role: String,
    pub cert_epoch: i64,
    pub cert_expires_at: i64,
    pub overlay_ip: String,
    pub mesh_id: String,
}

// ----- merged shape the UI binds against -----------------------

/// One row in the mesh-first peer list — Nebula peer reachability
/// + overlay identity.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MeshPeer {
    /// Stable peer node-id (Nebula's `node_id`).
    pub node_id: String,
    /// Display name (Nebula's `name`, fallback hostname).
    pub host: String,
    /// 10.42.x.x — Nebula overlay IP.
    pub overlay_ip: String,
    /// "online" / "idle" / "offline" — from Nebula.
    pub reachable: String,
    /// Last cert epoch the lighthouse signed for this peer.
    pub cert_epoch: i64,
}

// ----- backend client ------------------------------------------

/// Cheap-to-construct mesh-backend client. A tokio runtime + the
/// resolved Bus data dir are held; each call opens a fresh
/// `Persist` inside `rt.block_on` and blocks the caller until
/// mackesd's responder replies (with a per-call timeout so the UI
/// thread never freezes).
pub struct MeshBackend {
    rt: Runtime,
    /// Bus data dir. A fresh `Persist` is opened from it per call
    /// rather than held here, because `Persist` (rusqlite) is not
    /// `Send` and `MeshBackend` lives inside the UI backend.
    bus_dir: PathBuf,
    /// Per-call timeout — keeps the GUI thread snappy when
    /// mackesd is busy.
    call_timeout: Duration,
}

impl MeshBackend {
    /// Default connect timeout.
    pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_millis(800);
    /// Default per-method call timeout.
    pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_millis(750);

    /// Resolve the Bus data dir + verify mackesd's Nebula responder
    /// is live with a single round-trip probe. Preserves the old
    /// fast-fail contract callers expect (`Err` => `mesh = None` =>
    /// the backend falls back to Fleet.Files / local), but over the
    /// Bus rather than a D-Bus `NameHasOwner` check.
    pub fn connect_with_timeout(timeout: Duration) -> Result<Self, MeshError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| MeshError::Unavailable(format!("tokio runtime: {e}")))?;
        let bus_dir = mde_bus::default_data_dir()
            .ok_or_else(|| MeshError::Unavailable("no Bus data dir".into()))?;
        // Liveness probe — one `action/nebula/status` round-trip. If
        // mackesd's responder isn't up it times out, mirroring the
        // old NameHasOwner fast-fail so callers get `mesh = None`.
        rt.block_on(async {
            let persist = mde_bus::persist::Persist::open(bus_dir.clone())
                .map_err(|e| MeshError::Unavailable(format!("bus persist: {e}")))?;
            mde_bus::rpc::request(
                &persist,
                "action/nebula/status",
                mde_bus::hooks::config::Priority::Default,
                None,
                None,
                timeout,
            )
            .await
            .map(|_| ())
            .map_err(|e| MeshError::Unavailable(format!("nebula probe: {e}")))
        })?;
        Ok(Self {
            rt,
            bus_dir,
            call_timeout: Self::DEFAULT_CALL_TIMEOUT,
        })
    }

    /// Convenience — connect with the default timeout.
    pub fn connect() -> Result<Self, MeshError> {
        Self::connect_with_timeout(Self::DEFAULT_CONNECT_TIMEOUT)
    }

    /// Override the per-call timeout. Tests use this to keep the
    /// suite fast.
    #[must_use]
    pub fn with_call_timeout(mut self, t: Duration) -> Self {
        self.call_timeout = t;
        self
    }

    /// Live Nebula overlay status.
    pub fn nebula_status(&self) -> Result<NebulaStatus, MeshError> {
        let raw = self.bus_request("status")?;
        parse_nebula_status(&raw).ok_or_else(|| MeshError::Decode(format!("nebula_status: {raw}")))
    }

    /// Live Nebula peer roster.
    pub fn nebula_peers(&self) -> Result<Vec<NebulaPeer>, MeshError> {
        let raw = self.bus_request("list-peers")?;
        parse_nebula_peers(&raw).ok_or_else(|| MeshError::Decode(format!("nebula_peers: {raw}")))
    }

    /// Live Nebula self-node snapshot.
    pub fn nebula_self_node(&self) -> Result<NebulaSelfNode, MeshError> {
        let raw = self.bus_request("self-node")?;
        parse_nebula_self_node(&raw)
            .ok_or_else(|| MeshError::Decode(format!("nebula_self_node: {raw}")))
    }

    /// Live peer list — Nebula peer roster converted to the mesh shape.
    pub fn mesh_peers(&self) -> Result<Vec<MeshPeer>, MeshError> {
        let peers = self.nebula_peers().unwrap_or_default();
        Ok(peers.into_iter().map(nebula_peer_to_mesh_peer).collect())
    }

    /// Publish one `action/nebula/<verb>` request on the Bus + block
    /// for the reply body. A fresh `Persist` is opened per call (it
    /// isn't `Send`); `call_timeout` bounds the wait. `Err` on
    /// timeout / no-responder / empty reply — callers map that to
    /// their fallback path, exactly as the old D-Bus errors did.
    fn bus_request(&self, verb: &str) -> Result<String, MeshError> {
        let topic = format!("action/nebula/{verb}");
        self.rt.block_on(async {
            let persist = mde_bus::persist::Persist::open(self.bus_dir.clone())
                .map_err(|e| MeshError::Unavailable(format!("bus persist: {e}")))?;
            match mde_bus::rpc::request(
                &persist,
                &topic,
                mde_bus::hooks::config::Priority::Default,
                None,
                None,
                self.call_timeout,
            )
            .await
            {
                Ok(reply) => reply
                    .body
                    .ok_or_else(|| MeshError::Unavailable(format!("{topic}: empty reply"))),
                Err(e) => Err(MeshError::Unavailable(format!("{topic}: {e}"))),
            }
        })
    }
}

// ----- pure helpers --------------------------------------------

/// Parse the JSON returned by `dev.mackes.MDE.Nebula.Status::Status`.
#[must_use]
pub fn parse_nebula_status(raw: &str) -> Option<NebulaStatus> {
    serde_json::from_str(raw).ok()
}

/// Parse the JSON returned by `dev.mackes.MDE.Nebula.Status::ListPeers`.
#[must_use]
pub fn parse_nebula_peers(raw: &str) -> Option<Vec<NebulaPeer>> {
    serde_json::from_str(raw).ok()
}

/// Parse the JSON returned by `dev.mackes.MDE.Nebula.Status::SelfNode`.
#[must_use]
pub fn parse_nebula_self_node(raw: &str) -> Option<NebulaSelfNode> {
    serde_json::from_str(raw).ok()
}

/// Convert a `NebulaPeer` into the UI-facing `MeshPeer` shape.
#[must_use]
pub fn nebula_peer_to_mesh_peer(n: NebulaPeer) -> MeshPeer {
    MeshPeer {
        node_id: n.node_id,
        host: n.name,
        overlay_ip: n.overlay_ip,
        reachable: n.reachable,
        cert_epoch: n.cert_epoch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_nebula_status_decodes_real_shape() {
        let raw = r#"{
            "is_lighthouse": true,
            "ca_epoch": 3,
            "peer_count": 5,
            "mesh_id": "mesh-abc",
            "active_transport": "nebula_direct"
        }"#;
        let s = parse_nebula_status(raw).expect("parse");
        assert!(s.is_lighthouse);
        assert_eq!(s.ca_epoch, 3);
        assert_eq!(s.peer_count, 5);
        assert_eq!(s.mesh_id, "mesh-abc");
    }

    #[test]
    fn parse_nebula_status_returns_none_on_garbage() {
        assert!(parse_nebula_status("not json").is_none());
    }

    #[test]
    fn parse_nebula_peers_decodes_array() {
        let raw = r#"[
            {"node_id":"peer:pine","name":"pine","overlay_ip":"10.42.0.5","fingerprint":"abc","cert_epoch":3,"cert_expires_at":0,"reachable":"online"},
            {"node_id":"peer:birch","name":"birch","overlay_ip":"10.42.0.6","fingerprint":"def","cert_epoch":3,"cert_expires_at":0,"reachable":"offline"}
        ]"#;
        let rows = parse_nebula_peers(raw).expect("parse");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].overlay_ip, "10.42.0.5");
        assert_eq!(rows[1].reachable, "offline");
    }

    #[test]
    fn parse_nebula_self_node_decodes() {
        let raw = r#"{
            "node_id":"peer:anvil","host":"anvil","role":"host",
            "cert_epoch":3,"cert_expires_at":1234567890,
            "overlay_ip":"10.42.0.1","mesh_id":"mesh-abc"
        }"#;
        let s = parse_nebula_self_node(raw).expect("parse");
        assert_eq!(s.overlay_ip, "10.42.0.1");
        assert_eq!(s.role, "host");
    }

    #[test]
    fn nebula_peer_to_mesh_peer_maps_fields() {
        let n = NebulaPeer {
            node_id: "peer:pine".into(),
            name: "pine".into(),
            overlay_ip: "10.42.0.5".into(),
            fingerprint: "f".into(),
            cert_epoch: 3,
            cert_expires_at: 0,
            reachable: "online".into(),
        };
        let mp = nebula_peer_to_mesh_peer(n);
        assert_eq!(mp.node_id, "peer:pine");
        assert_eq!(mp.host, "pine");
        assert_eq!(mp.overlay_ip, "10.42.0.5");
        assert_eq!(mp.reachable, "online");
        assert_eq!(mp.cert_epoch, 3);
    }

    #[test]
    fn mesh_error_display_carries_context() {
        let e = MeshError::Unavailable("session bus closed".into());
        assert!(format!("{e}").contains("session bus closed"));
        let e = MeshError::Decode("bad JSON".into());
        assert!(format!("{e}").contains("bad JSON"));
    }
}
