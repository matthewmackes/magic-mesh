//! TRANSFERS-7 — auto destination registry.
//!
//! Destinations are a projection, not a second database: mesh peers come from the
//! replicated [`PeerRecord`] directory, and standing service destinations are
//! derived from node-state in that same workgroup root. Ad-hoc URL/SFTP hosts stay
//! per-job inputs and are deliberately absent from this registry.

#![cfg(feature = "async-services")]

use std::path::Path;

use mackes_mesh_types::peers::{peers_dir, read_peers, PeerRecord};
use serde::{Deserialize, Serialize};

use super::job::Method;

/// Stable destination categories the GUI/CLI can render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationKind {
    /// A mesh peer from the replicated peer roster.
    MeshPeer,
    /// The canonical Syncthing/QNM shared directory.
    MeshShare,
    /// The standing Navidrome music-library target.
    MusicLibrary,
}

/// One auto-discovered destination row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferDestination {
    /// Stable row id for pinning/rendering.
    pub id: String,
    /// Human label shown by renderer surfaces.
    pub label: String,
    /// Destination category.
    pub kind: DestinationKind,
    /// Preferred transfer method when a user chooses this destination.
    pub method: Method,
    /// The destination string copied onto [`super::TransferJob::dest`].
    pub dest: String,
    /// Optional mesh overlay IP, for peer rows that advertise one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_ip: Option<String>,
}

/// Discover all auto destinations from the replicated node-state plane.
#[must_use]
pub fn discover_destinations(
    workgroup_root: &Path,
    self_host: Option<&str>,
) -> Vec<TransferDestination> {
    let peers = read_peers(&peers_dir(workgroup_root));
    destinations_from_state(workgroup_root, &peers, self_host)
}

/// Pure fold for tests and future Bus publication.
#[must_use]
pub fn destinations_from_state(
    workgroup_root: &Path,
    peers: &[PeerRecord],
    self_host: Option<&str>,
) -> Vec<TransferDestination> {
    let mut out = Vec::new();
    out.push(mesh_share_destination(workgroup_root));
    if has_music_library(peers) {
        out.push(music_library_destination());
    }
    let self_host = self_host.map(normalize_host);
    for peer in peers {
        if peer.health == "unreachable" || peer.is_stale(stale_peer_ms()) {
            continue;
        }
        let host = normalize_host(&peer.hostname);
        if self_host.as_deref() == Some(host.as_str()) {
            continue;
        }
        if host.is_empty() {
            continue;
        }
        out.push(TransferDestination {
            id: format!("peer:{host}"),
            label: peer_label(peer, &host),
            kind: DestinationKind::MeshPeer,
            method: Method::Node,
            dest: format!("node:{host}"),
            overlay_ip: peer.overlay_ip.clone(),
        });
    }
    out.sort_by(|a, b| {
        destination_rank(a.kind)
            .cmp(&destination_rank(b.kind))
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Per-job external endpoints are intentionally not registry destinations.
#[must_use]
pub fn is_ad_hoc_endpoint(raw: &str) -> bool {
    let s = raw.trim();
    s.contains("://")
        || (s.contains(':') && !s.starts_with('/') && !s.starts_with("./") && !s.starts_with("../"))
}

fn mesh_share_destination(workgroup_root: &Path) -> TransferDestination {
    TransferDestination {
        id: "mesh-share".into(),
        label: "Mesh Share".into(),
        kind: DestinationKind::MeshShare,
        method: Method::Node,
        dest: workgroup_root.display().to_string(),
        overlay_ip: None,
    }
}

fn music_library_destination() -> TransferDestination {
    let library = crate::default_qnm_shared_root().join("music-library");
    TransferDestination {
        id: "music-library".into(),
        label: "Music Library".into(),
        kind: DestinationKind::MusicLibrary,
        method: Method::Music,
        dest: library.display().to_string(),
        overlay_ip: None,
    }
}

fn has_music_library(peers: &[PeerRecord]) -> bool {
    peers
        .iter()
        .any(|p| p.media && p.role.as_deref() == Some("lighthouse"))
}

fn peer_label(peer: &PeerRecord, fallback: &str) -> String {
    if peer.role.as_deref() == Some("lighthouse") {
        format!("{fallback} Lighthouse")
    } else {
        fallback.to_string()
    }
}

fn normalize_host(raw: &str) -> String {
    raw.trim().trim_start_matches("peer:").to_ascii_lowercase()
}

fn destination_rank(kind: DestinationKind) -> u8 {
    match kind {
        DestinationKind::MeshShare => 0,
        DestinationKind::MusicLibrary => 1,
        DestinationKind::MeshPeer => 2,
    }
}

fn stale_peer_ms() -> u64 {
    // Match the broad peer-directory convention: a missing or very old heartbeat
    // should not be offered as an active transfer target.
    10 * 60 * 1_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::peers::write_peer_record;
    use std::path::PathBuf;

    fn peer(host: &str, ip: Option<&str>) -> PeerRecord {
        let mut rec = PeerRecord::now(host, Some("12.0.0".into()), "healthy");
        rec.overlay_ip = ip.map(str::to_string);
        rec.last_seen_ms = crate::workers::transfers::now_ms();
        rec
    }

    #[test]
    fn registry_derives_mesh_share_music_and_peer_destinations() {
        let root = PathBuf::from("/mnt/mesh-storage");
        let mut media = peer("lh-media", Some("10.42.0.10"));
        media.role = Some("lighthouse".into());
        media.media = true;
        let peers = vec![
            peer("self-node", Some("10.42.0.1")),
            media,
            peer("workstation-b", Some("10.42.0.21")),
        ];
        let rows = destinations_from_state(&root, &peers, Some("self-node"));
        assert_eq!(rows[0].id, "mesh-share");
        assert_eq!(rows[0].dest, "/mnt/mesh-storage");
        assert!(rows.iter().any(|d| {
            d.id == "music-library"
                && d.kind == DestinationKind::MusicLibrary
                && d.method == Method::Music
                && d.dest.ends_with("/music-library")
        }));
        assert!(rows.iter().any(|d| {
            d.id == "peer:workstation-b"
                && d.kind == DestinationKind::MeshPeer
                && d.method == Method::Node
                && d.dest == "node:workstation-b"
                && d.overlay_ip.as_deref() == Some("10.42.0.21")
        }));
        assert!(!rows.iter().any(|d| d.id == "peer:self-node"));
    }

    #[test]
    fn registry_drops_stale_and_unreachable_peers() {
        let mut stale = peer("stale", Some("10.42.0.8"));
        stale.last_seen_ms = 1;
        let mut unreachable = peer("down", Some("10.42.0.9"));
        unreachable.health = "unreachable".into();
        let rows = destinations_from_state(Path::new("/wg"), &[stale, unreachable], None);
        assert_eq!(
            rows.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            ["mesh-share"]
        );
    }

    #[test]
    fn discover_reads_the_peer_directory_without_saving_ad_hoc_hosts() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = peers_dir(tmp.path());
        let mut rec = peer("oak", Some("10.42.0.44"));
        rec.role = Some("workstation".into());
        write_peer_record(&dir, &rec).unwrap();
        let rows = discover_destinations(tmp.path(), None);
        assert!(rows.iter().any(|d| d.id == "peer:oak"));
        assert!(is_ad_hoc_endpoint("sftp.example.com:/srv/drop"));
        assert!(is_ad_hoc_endpoint("https://example.invalid/file.bin"));
        assert!(!rows.iter().any(|d| d.id.contains("example")));
    }
}
