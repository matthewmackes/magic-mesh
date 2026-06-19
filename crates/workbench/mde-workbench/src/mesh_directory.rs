//! SUBSTRATE-8 (SUBSTRATE-V2) — read the mesh peer directory over the Bus RPC.
//!
//! The panels used to read the peer roster straight off `/mnt/mesh-storage`
//! (`peers::read_peers`). Post-cutover that path is a Syncthing folder and the
//! peer records live in etcd, so a direct-FS read returns nothing. This helper
//! routes every panel through `action/mesh/directory` instead — whose mackesd
//! responder reads etcd-or-fs (the SUBSTRATE-3 bridge), so the panels become
//! substrate-agnostic and keep working across the cutover. `mackes_mesh_types::
//! peers::PeerRecord` stays the in-GUI shape, rebuilt from the RPC reply.

use std::time::Duration;

use mackes_mesh_types::peers::PeerRecord;

/// Read budget for the directory probe — matches the peers panel's 2 s.
const DIRECTORY_TIMEOUT: Duration = Duration::from_secs(2);

/// Parse an `action/mesh/directory` reply (`{ ok, peers: [...] }`) into
/// `PeerRecord`s — the fields the panels render (hostname, overlay IP, health,
/// role, last-seen, version). Pure + testable. A non-ok / unparseable reply, or
/// a missing `peers` array, yields an empty list (an honest empty directory).
#[must_use]
pub fn parse_directory_peers(reply: &str) -> Vec<PeerRecord> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(reply.trim()) else {
        return Vec::new();
    };
    if v.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Vec::new();
    }
    let Some(peers) = v.get("peers").and_then(|p| p.as_array()) else {
        return Vec::new();
    };
    peers
        .iter()
        .filter_map(|p| {
            let hostname = p.get("hostname").and_then(serde_json::Value::as_str)?;
            if hostname.is_empty() {
                return None;
            }
            let mut rec = PeerRecord::now(
                hostname,
                p.get("mde_version")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                p.get("health")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown"),
            );
            rec.last_seen_ms = p
                .get("last_seen_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(rec.last_seen_ms);
            rec.overlay_ip = p
                .get("overlay_ip")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            rec.role = p
                .get("role")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            Some(rec)
        })
        .collect()
}

/// Fetch the live peer directory over the Bus (`action/mesh/directory`). Empty on
/// a daemon-down / timeout / no-responder (the panels render an honest empty
/// state). Blocking — call from `tokio::task::spawn_blocking` on the iced
/// executor (the [`crate::dbus::action_request`] current-thread-runtime contract).
#[must_use]
pub fn fetch_peers() -> Vec<PeerRecord> {
    match crate::dbus::action_request("action/mesh/directory", DIRECTORY_TIMEOUT) {
        Some(reply) => parse_directory_peers(&reply),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_directory_peers_decodes_the_rpc_shape() {
        // The exact shape mackesd's action/mesh/directory responder emits.
        let reply = r#"{"ok":true,"head":7,"peers":[
            {"hostname":"node-a","presence":"online","last_seen_ms":111,
             "health":"healthy","mde_version":"11.0.0","overlay_ip":"10.42.0.2",
             "role":"server","tags":[]},
            {"hostname":"node-b","presence":"offline","last_seen_ms":222,
             "health":"degraded","mde_version":null,"overlay_ip":"","role":null,"tags":[]}
        ]}"#;
        let peers = parse_directory_peers(reply);
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].hostname, "node-a");
        assert_eq!(peers[0].overlay_ip.as_deref(), Some("10.42.0.2"));
        assert_eq!(peers[0].health, "healthy");
        assert_eq!(peers[0].role.as_deref(), Some("server"));
        assert_eq!(peers[0].mde_version.as_deref(), Some("11.0.0"));
        assert_eq!(peers[0].last_seen_ms, 111);
        // Empty overlay_ip / null role → None (not Some("")).
        assert_eq!(peers[1].overlay_ip, None);
        assert_eq!(peers[1].role, None);
        assert_eq!(peers[1].health, "degraded");
    }

    #[test]
    fn parse_directory_peers_handles_error_and_garbage() {
        assert!(parse_directory_peers(r#"{"error":"boom"}"#).is_empty());
        assert!(parse_directory_peers(r#"{"ok":false}"#).is_empty());
        assert!(parse_directory_peers("not json").is_empty());
        assert!(parse_directory_peers(r#"{"ok":true}"#).is_empty()); // no peers key
        assert!(parse_directory_peers(r#"{"ok":true,"peers":[]}"#).is_empty());
    }

    #[test]
    fn parse_directory_peers_skips_rows_without_hostname() {
        let reply =
            r#"{"ok":true,"peers":[{"health":"healthy"},{"hostname":"x","health":"healthy"}]}"#;
        let peers = parse_directory_peers(reply);
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].hostname, "x");
    }
}
