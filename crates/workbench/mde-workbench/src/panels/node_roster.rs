//! Node-roster helper — `mackesd nodes list --json` → typed rows.
//!
//! PD-7 — extracted from the retired **Mesh Topology** panel (whose graph
//! the PD-7 `peers_map` reborn replaced). The Home capability row is the
//! only consumer now; the data fetch + parse live here so the dead panel
//! could be deleted without losing them.

use serde_json::Value;

/// A peer's coarse presence, mapped from the node's `health`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerStatus {
    Online,
    Idle,
    Offline,
    Unknown,
}

impl PeerStatus {
    fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "online" | "healthy" => Self::Online,
            "idle" | "degraded" => Self::Idle,
            "offline" | "unreachable" => Self::Offline,
            _ => Self::Unknown,
        }
    }
}

/// One node-roster row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRow {
    pub name: String,
    pub addr: String,
    pub kind: String,
    pub status: PeerStatus,
}

/// Shell `mackesd nodes list --json` and parse the roster.
///
/// # Errors
/// Spawn failure or a non-zero exit (with the captured stderr).
pub fn fetch_peers() -> Result<Vec<PeerRow>, String> {
    let out = std::process::Command::new("mackesd")
        .args(["nodes", "list", "--json"])
        .output()
        .map_err(|e| format!("mackesd nodes list failed to spawn: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("mackesd nodes list exited non-zero: {stderr}"));
    }
    Ok(parse_nodes(&String::from_utf8_lossy(&out.stdout)))
}

/// Pure parser for `mackesd nodes list --json`'s array output. Each entry
/// is `{node_id, name, public_key, role, health, region}` per
/// `mackesd_core::store::NodeRow`. Sorted by name; junk-tolerant.
#[must_use]
pub fn parse_nodes(raw: &str) -> Vec<PeerRow> {
    let Ok(top) = serde_json::from_str::<Vec<Value>>(raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in top {
        let node_id = entry.get("node_id").and_then(|v| v.as_str()).unwrap_or("");
        if node_id.is_empty() {
            continue;
        }
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(node_id);
        let region = entry.get("region").and_then(|v| v.as_str()).unwrap_or("—");
        let role = entry.get("role").and_then(|v| v.as_str()).unwrap_or("peer");
        let health = entry
            .get("health")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        out.push(PeerRow {
            name: name.to_string(),
            addr: region.to_string(),
            kind: role.to_string(),
            status: PeerStatus::from_str(health),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_nodes_maps_fields_and_health_and_sorts() {
        let raw = r#"[
            {"node_id":"n2","name":"oak","role":"peer","health":"degraded","region":"us-w"},
            {"node_id":"n1","name":"anvil","role":"lighthouse","health":"healthy","region":"us-e"},
            {"node_id":"","name":"ghost","role":"peer","health":"healthy"}
        ]"#;
        let rows = parse_nodes(raw);
        assert_eq!(rows.len(), 2, "empty node_id is dropped");
        assert_eq!(rows[0].name, "anvil", "sorted by name");
        assert_eq!(rows[0].kind, "lighthouse");
        assert_eq!(rows[0].status, PeerStatus::Online);
        assert_eq!(rows[1].status, PeerStatus::Idle);
    }

    #[test]
    fn parse_nodes_is_junk_tolerant() {
        assert!(parse_nodes("not json").is_empty());
        assert!(parse_nodes("").is_empty());
    }
}
