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
    /// Loaded from `$MDE_VOICE_HUD_ROSTER`.
    EnvOverride,
    /// Loaded from `<mesh_storage_home>/voip/peers.toml`.
    MeshStorage,
    /// Loaded from `~/.config/mde/voip/peers.toml`.
    LocalFallback,
    /// Loaded from the compile-time embedded fixture.
    EmbeddedFixture,
}

impl RosterSource {
    /// Short label for the topbar source chip.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            RosterSource::EnvOverride => "roster · env",
            RosterSource::MeshStorage => "roster · mesh-storage",
            RosterSource::LocalFallback => "roster · local",
            RosterSource::EmbeddedFixture => "roster · fixture",
        }
    }
}

/// Try every roster source in order, returning the first hit.
#[must_use]
pub fn load() -> RosterLoad {
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
/// Honors `$MDE_MESH_STORAGE_HOME` if set; otherwise falls back to
/// `/mnt/mesh-storage` (the MESHFS v5.0 default mount target).
fn mesh_storage_home() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MDE_MESH_STORAGE_HOME") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let p = PathBuf::from("/mnt/mesh-storage");
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
