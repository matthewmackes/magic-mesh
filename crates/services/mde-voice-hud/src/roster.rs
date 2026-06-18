//! Peer roster loader for `mde-voice-hud`.
//!
//! Resolves the active mesh peer roster from the first available
//! source:
//!
//! 1. `$MDE_VOICE_HUD_ROSTER` — explicit override path (dev/testing).
//! 2. `<mesh_storage_home>/voip/peers.toml` — the live VOIP-26
//!    Auto-Connect roster, LizardFS-replicated across the mesh.
//! 3. `~/.config/mde/voip/peers.toml` — per-peer local fallback,
//!    refreshed every time the mesh-storage read succeeds.
//! 4. Embedded compile-time fixture (`test-fixtures/roster.toml`).
//!
//! Returning the embedded fixture as the last resort matches
//! VOIP-27's acceptance: the Peers tab renders 8 rows even when
//! launched outside a fully-booted mded session (e.g.,
//! `cargo run -p mde-voice-hud`).

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

/// Compile-time embedded fallback. Sourced from
/// `test-fixtures/roster.toml`.
const EMBEDDED_FIXTURE: &str = include_str!("../test-fixtures/roster.toml");

/// A single peer in the mesh roster.
///
/// Matches the design bundle's `PEERS` row shape (ext / name / role /
/// presence / lan / hint).
#[derive(Debug, Clone, Deserialize)]
pub struct Peer {
    /// Mesh extension `1NNN` (per VOIP-3 bare-hostname dialing).
    pub ext: String,
    /// Mesh hostname (matches `<peer>.mesh.mde` DNS).
    pub name: String,
    /// Mesh role tag — `"GUI"` / `"Host"` / `"Node"`.
    pub role: String,
    /// Last-known presence — one of available/on-call/away/dnd/offline.
    pub presence: String,
    /// `true` when the peer is reachable on LAN (no Nebula WAN hop).
    pub lan: bool,
    /// Short human-readable hint (e.g., "Alice's ThinkPad").
    pub hint: String,
}

#[derive(Debug, Deserialize)]
struct RosterFile {
    peers: Vec<Peer>,
}

/// Result of a roster-load attempt — also carries the source for
/// the topbar status string.
pub struct RosterLoad {
    /// Loaded peer list (may be empty if every source fails — but
    /// the embedded fixture always succeeds, so an empty result
    /// only happens if parsing breaks).
    pub peers: Vec<Peer>,
    /// Where the roster came from (used to tell the operator
    /// whether they're on live data or fixture).
    pub source: RosterSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RosterSource {
    /// VOIP-P2P-3 — the live mesh directory (`<workgroup>/peers/*.json`): every
    /// reachable peer, dialable directly over the overlay by name.
    MeshDirectory,
    /// Loaded from `$MDE_VOICE_HUD_ROSTER`.
    EnvOverride,
    /// Loaded from `<mesh_storage_home>/voip/peers.toml`.
    MeshStorage,
    /// Loaded from `~/.config/mde/voip/peers.toml`.
    LocalFallback,
    /// Loaded from the compile-time embedded fixture.
    EmbeddedFixture,
}

/// VOIP-P2P-3 — map a live directory [`PeerRecord`] to a dialable HUD row. For
/// registrar-less P2P the peer is dialed by name (no extension), so `ext` is
/// empty; `health` drives the presence pip. Pure + testable.
#[must_use]
fn record_to_peer(r: mackes_mesh_types::peers::PeerRecord) -> Peer {
    let presence = match r.health.as_str() {
        "healthy" => "available",
        "unreachable" | "critical" => "offline",
        "degraded" => "away",
        _ => "available",
    }
    .to_string();
    let hint = r
        .overlay_ip
        .clone()
        .unwrap_or_else(|| "mesh peer".to_string());
    Peer {
        ext: String::new(),
        name: r.hostname,
        role: "Node".to_string(),
        presence,
        lan: false,
        hint,
    }
}

/// VOIP-P2P-3 — the live directory as the dialable roster: read every
/// `peers/*.json` under the workgroup mount, excluding this node itself.
/// `None` when the mount is absent / the directory is empty.
fn directory_peers() -> Option<Vec<Peer>> {
    let home = mesh_storage_home()?;
    let dir = mackes_mesh_types::peers::peers_dir(&home);
    let self_host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let peers: Vec<Peer> = mackes_mesh_types::peers::read_peers(&dir)
        .into_iter()
        .filter(|r| r.hostname != self_host)
        .map(record_to_peer)
        .collect();
    (!peers.is_empty()).then_some(peers)
}

/// Try every roster source in order, returning the first hit.
#[must_use]
pub fn load() -> RosterLoad {
    // VOIP-P2P-3 — the live mesh directory is the preferred dialable roster.
    if let Some(peers) = directory_peers() {
        tracing::info!(
            count = peers.len(),
            "loaded roster from live mesh directory"
        );
        return RosterLoad {
            peers,
            source: RosterSource::MeshDirectory,
        };
    }

    if let Ok(path) = std::env::var("MDE_VOICE_HUD_ROSTER") {
        if let Some(peers) = try_load(&PathBuf::from(&path)) {
            tracing::info!(path, count = peers.len(), "loaded roster from env override");
            return RosterLoad {
                peers,
                source: RosterSource::EnvOverride,
            };
        }
    }

    if let Some(home) = mesh_storage_home() {
        let p = home.join("voip/peers.toml");
        if let Some(peers) = try_load(&p) {
            tracing::info!(path = %p.display(), count = peers.len(), "loaded roster from mesh-storage");
            return RosterLoad {
                peers,
                source: RosterSource::MeshStorage,
            };
        }
    }

    if let Some(cfg) = dirs::config_dir() {
        let p = cfg.join("mde/voip/peers.toml");
        if let Some(peers) = try_load(&p) {
            tracing::info!(path = %p.display(), count = peers.len(), "loaded roster from local fallback");
            return RosterLoad {
                peers,
                source: RosterSource::LocalFallback,
            };
        }
    }

    let peers = parse(EMBEDDED_FIXTURE).expect("embedded fixture must parse");
    tracing::info!(count = peers.len(), "loaded roster from embedded fixture");
    RosterLoad {
        peers,
        source: RosterSource::EmbeddedFixture,
    }
}

fn try_load(path: &PathBuf) -> Option<Vec<Peer>> {
    let body = fs::read_to_string(path).ok()?;
    parse(&body).ok()
}

fn parse(body: &str) -> Result<Vec<Peer>, toml::de::Error> {
    toml::from_str::<RosterFile>(body).map(|f| f.peers)
}

/// Best-effort resolution of `<mesh_storage_home>` per the
/// [[project_v5_0_0_lizardfs_mesh_storage]] FUSE-mount convention.
///
/// Honors `$MDE_MESH_STORAGE_HOME` if set; otherwise resolves the
/// QNM-Shared mount the same way `mackesd` does (`default_workgroup_root`
/// — `~/QNM-Shared` by default), so the roster reads the mount the
/// daemon actually writes rather than a phantom `/mnt/mesh-storage`.
fn mesh_storage_home() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MDE_MESH_STORAGE_HOME") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let p = mackes_mesh_types::peers::default_workgroup_root();
    p.exists().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fixture_parses_to_eight_peers() {
        let peers = parse(EMBEDDED_FIXTURE).expect("fixture parses");
        assert_eq!(peers.len(), 8);
        assert!(peers.iter().any(|p| p.ext == "1004" && p.role == "Host"));
    }

    #[test]
    fn record_to_peer_maps_health_and_is_registrar_less() {
        let rec = mackes_mesh_types::peers::PeerRecord {
            hostname: "pine".into(),
            mde_version: None,
            last_seen_ms: 0,
            health: "healthy".into(),
            descriptors: None,
            overlay_ip: Some("10.42.0.7".into()),
            role: None,
        };
        let p = record_to_peer(rec);
        assert_eq!(p.name, "pine");
        assert!(p.ext.is_empty(), "P2P dials by name, no extension");
        assert_eq!(p.presence, "available");
        assert_eq!(p.hint, "10.42.0.7");

        let unreachable = mackes_mesh_types::peers::PeerRecord {
            hostname: "oak".into(),
            mde_version: None,
            last_seen_ms: 0,
            health: "unreachable".into(),
            descriptors: None,
            overlay_ip: None,
            role: None,
        };
        assert_eq!(record_to_peer(unreachable).presence, "offline");
    }

    #[test]
    fn fallback_loads_embedded_when_no_sources() {
        // Ensure the env override + GFS + local paths are all unset
        // for this test process. (Test harness inherits env; we
        // unset the override + use a dirs::config_dir that almost
        // certainly doesn't contain a real peers.toml.)
        std::env::remove_var("MDE_VOICE_HUD_ROSTER");
        std::env::remove_var("MDE_MESH_STORAGE_HOME");
        let r = load();
        assert!(!r.peers.is_empty());
    }
}
