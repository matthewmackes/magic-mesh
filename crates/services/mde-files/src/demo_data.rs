//! Demo data — the same dummy roster the prototype shipped,
//! migrated from `const &[Foo]` to `pub fn foo() -> Vec<Foo>`
//! as part of the v4.0.1 AF-* mega (Phase G) so the model
//! structs can carry `String` instead of `&'static str` and
//! `DBusBackend` / `LocalFsBackend` can return values through
//! the same types.
//!
//! Two consumers remain:
//!   * `DemoBackend` — read-only, used for unit tests + the
//!     "panel boots in 200 ms" smoke gate when no `mackesd` is
//!     reachable.
//!   * `tests/*` — assertion fixtures.
//!
//! The runtime binary now constructs `RealBackend` (see
//! `backend.rs`) which talks to `mackesd` for mesh content +
//! reads the local filesystem for Local content — it does NOT
//! pull from this file at all. If `mackesd` is unreachable the
//! peers list comes back empty and the sidebar's MESH section
//! renders the curated "no peers connected" empty state.

use crate::model::{
    FileRow, LocalPin, Mime, Peer, PeerKind, PeerStatus, PinIcon, SelfNode, Transfer, TxDir,
};

#[must_use]
pub fn self_node() -> SelfNode {
    SelfNode {
        id: "yew".into(),
        host: "yew.mesh".into(),
        label: "this node".into(),
        addr: "10.0.7.1".into(),
        files: 1284,
        shared: 38,
    }
}

#[must_use]
pub fn peers() -> Vec<Peer> {
    vec![
        Peer {
            id: "pine".into(),
            host: "pine.mesh".into(),
            label: "matthew · workstation".into(),
            kind: PeerKind::Desktop,
            addr: "10.0.7.14".into(),
            status: PeerStatus::Online,
            latency: Some(14),
            files: 4912,
            shared: 211,
            last: "now".into(),
            derp: "fra".into(),
        },
        Peer {
            id: "birch".into(),
            host: "birch.mesh".into(),
            label: "home server · NAS".into(),
            kind: PeerKind::Server,
            addr: "10.0.7.22".into(),
            status: PeerStatus::Online,
            latency: Some(41),
            files: 18_403,
            shared: 1842,
            last: "12 s".into(),
            derp: "ord".into(),
        },
        Peer {
            id: "oak".into(),
            host: "oak.mesh".into(),
            label: "matt-phone".into(),
            kind: PeerKind::Phone,
            addr: "10.0.7.41".into(),
            status: PeerStatus::Idle,
            latency: Some(220),
            files: 612,
            shared: 4,
            last: "3 min".into(),
            derp: "fra".into(),
        },
        Peer {
            id: "cedar".into(),
            host: "cedar.mesh".into(),
            label: "CI · build runner".into(),
            kind: PeerKind::Server,
            addr: "10.0.7.51".into(),
            status: PeerStatus::Offline,
            latency: None,
            files: 0,
            shared: 0,
            last: "2 h ago".into(),
            derp: "—".into(),
        },
    ]
}

#[must_use]
pub fn recent_transfers() -> Vec<Transfer> {
    vec![
        Transfer {
            dir: TxDir::In,
            name: "map2-release-v0.4.2.tar.zst".into(),
            peer: "cedar.mesh".into(),
            size: "14.2 MB".into(),
            age: "12 s".into(),
        },
        Transfer {
            dir: TxDir::Out,
            name: "design-notes.md".into(),
            peer: "pine.mesh".into(),
            size: "8 KB".into(),
            age: "4 min".into(),
        },
        Transfer {
            dir: TxDir::In,
            name: "kitchen-IMG_5611.jpg".into(),
            peer: "oak.mesh".into(),
            size: "3.8 MB".into(),
            age: "14 min".into(),
        },
        Transfer {
            dir: TxDir::In,
            name: "projector-warranty.pdf".into(),
            peer: "birch.mesh".into(),
            size: "210 KB".into(),
            age: "1 h".into(),
        },
        Transfer {
            dir: TxDir::Out,
            name: "screenshots/2026-05-19.zip".into(),
            peer: "birch.mesh".into(),
            size: "22.1 MB".into(),
            age: "2 h".into(),
        },
    ]
}

#[must_use]
pub fn inbox() -> Vec<FileRow> {
    vec![
        FileRow::local(
            "map2-release-v0.4.2.tar.zst",
            Mime::Archive,
            "14.2 MB",
            "12 s",
        )
        .with_from("cedar.mesh"),
        FileRow::local("meeting-notes-2026-05-18.md", Mime::Doc, "4 KB", "6 min")
            .with_from("pine.mesh"),
        FileRow::local("kitchen-IMG_5611.jpg", Mime::Image, "3.8 MB", "14 min")
            .with_from("oak.mesh"),
        FileRow::local("projector-warranty.pdf", Mime::Pdf, "210 KB", "1 h")
            .with_from("birch.mesh"),
        FileRow::local("birch-photos-april/", Mime::Folder, "— · 412 items", "3 h")
            .with_from("birch.mesh"),
        FileRow::local("pine-clipboard.txt", Mime::Doc, "1 KB", "4 h").with_from("pine.mesh"),
    ]
}

#[must_use]
pub fn downloads() -> Vec<FileRow> {
    vec![
        FileRow::local(
            "cargo-1.87.0-x86_64-unknown-linux-gnu.tar.xz",
            Mime::Archive,
            "38.4 MB",
            "5 min",
        ),
        FileRow::local(
            "map2-release-v0.4.2.tar.zst",
            Mime::Archive,
            "14.2 MB",
            "12 s",
        )
        .with_mesh("cedar.mesh"),
        FileRow::local("fedora-coreos-aarch64.qcow2", Mime::Disk, "684 MB", "2 h"),
        FileRow::local("meeting-notes-2026-05-18.md", Mime::Doc, "4 KB", "6 min")
            .with_mesh("pine.mesh"),
        FileRow::local(
            "screenshot-2026-05-19-08-52-56.png",
            Mime::Image,
            "218 KB",
            "3 min",
        ),
        FileRow::local("kitchen-IMG_5611.jpg", Mime::Image, "3.8 MB", "14 min")
            .with_mesh("oak.mesh"),
        FileRow::local("projector-warranty.pdf", Mime::Pdf, "210 KB", "1 h")
            .with_mesh("birch.mesh"),
        FileRow::local("map2-panel-screenshot.png", Mime::Image, "512 KB", "20 min"),
        FileRow::local("rust-1.87.0-src.tar.gz", Mime::Archive, "186 MB", "1 d"),
    ]
}

#[must_use]
pub fn pine_files() -> Vec<FileRow> {
    vec![
        FileRow::local("~mesh/", Mime::Folder, "— · 38 items", "—"),
        FileRow::local("screenshots/", Mime::Folder, "— · 122 items", "—"),
        FileRow::local("design-notes.md", Mime::Doc, "8 KB", "4 min"),
        FileRow::local("meeting-notes-2026-05-18.md", Mime::Doc, "4 KB", "6 min"),
        FileRow::local("map2-panel-mockup.fig", Mime::Doc, "1.4 MB", "1 h"),
        FileRow::local("pine-clipboard.txt", Mime::Doc, "1 KB", "4 h"),
        FileRow::local("desktop.jpg", Mime::Image, "2.2 MB", "1 d"),
    ]
}

#[must_use]
pub fn birch_files() -> Vec<FileRow> {
    vec![
        FileRow::local("~mesh/", Mime::Folder, "— · 1842 items", "—"),
        FileRow::local("family-photos/", Mime::Folder, "— · 14.2k items", "—"),
        FileRow::local("media/", Mime::Folder, "— · 612 items", "—"),
        FileRow::local("backups/", Mime::Folder, "— · 211 items", "—"),
        FileRow::local("projector-warranty.pdf", Mime::Pdf, "210 KB", "1 h"),
        FileRow::local("fedora-coreos-aarch64.qcow2", Mime::Disk, "684 MB", "2 h"),
    ]
}

#[must_use]
pub fn oak_files() -> Vec<FileRow> {
    vec![
        FileRow::local("Camera/", Mime::Folder, "— · 412 items", "—"),
        FileRow::local("kitchen-IMG_5611.jpg", Mime::Image, "3.8 MB", "14 min"),
        FileRow::local("voice-memo-2026-05-19.m4a", Mime::Doc, "420 KB", "20 min"),
    ]
}

#[must_use]
pub fn local_pins() -> Vec<LocalPin> {
    vec![
        LocalPin {
            id: "home".into(),
            name: "Home".into(),
            path: "~/".into(),
            icon: PinIcon::Home,
        },
        LocalPin {
            id: "docs".into(),
            name: "Documents".into(),
            path: "~/Documents".into(),
            icon: PinIcon::Doc2,
        },
        LocalPin {
            id: "pics".into(),
            name: "Pictures".into(),
            path: "~/Pictures".into(),
            icon: PinIcon::Image,
        },
        LocalPin {
            id: "music".into(),
            name: "Music".into(),
            path: "~/Music".into(),
            icon: PinIcon::Doc,
        },
        LocalPin {
            id: "videos".into(),
            name: "Videos".into(),
            path: "~/Videos".into(),
            icon: PinIcon::Player,
        },
        LocalPin {
            id: "code".into(),
            name: "Code".into(),
            path: "~/code".into(),
            icon: PinIcon::Rust,
        },
        LocalPin {
            id: "root".into(),
            name: "Filesystem".into(),
            path: "/".into(),
            icon: PinIcon::Hdd,
        },
        LocalPin {
            id: "trash".into(),
            name: "Trash".into(),
            path: "empty".into(),
            icon: PinIcon::Trash,
        },
    ]
}

#[must_use]
pub fn local_recent() -> Vec<FileRow> {
    vec![
        FileRow::local(".bashrc", Mime::Doc, "3 KB", "2 h"),
        FileRow::local("Documents/journal.md", Mime::Doc, "14 KB", "5 h"),
        FileRow::local("code/map2/", Mime::Folder, "— · 312 items", "12 min"),
        FileRow::local("Pictures/wallpapers/", Mime::Folder, "— · 28 items", "1 d"),
    ]
}

/// Files shared by a peer, looked up by `peer.id`. Empty vec for
/// unknown ids.
#[must_use]
pub fn peer_files(id: &str) -> Vec<FileRow> {
    match id {
        "pine" => pine_files(),
        "birch" => birch_files(),
        "oak" => oak_files(),
        _ => Vec::new(),
    }
}

/// How many peers are online right now (used in banners + sidebar
/// header).
#[must_use]
pub fn online_count() -> usize {
    peers()
        .iter()
        .filter(|p| p.status == PeerStatus::Online)
        .count()
}

/// Sum of `shared` across self + all peers — the "Shared" stat in
/// the banner.
#[must_use]
pub fn total_shared() -> u64 {
    u64::from(self_node().shared) + peers().iter().map(|p| u64::from(p.shared)).sum::<u64>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_peers_three_active() {
        let p = peers();
        assert_eq!(p.len(), 4);
        let active = p
            .iter()
            .filter(|p| !matches!(p.status, PeerStatus::Offline))
            .count();
        assert_eq!(active, 3);
    }

    #[test]
    fn online_count_matches_prototype() {
        assert_eq!(online_count(), 2);
    }

    #[test]
    fn total_shared_matches_prototype() {
        assert_eq!(total_shared(), 38 + 211 + 1842 + 4 + 0);
    }

    #[test]
    fn downloads_mesh_count_matches_prototype() {
        let mesh_arrived = downloads().iter().filter(|d| d.mesh.is_some()).count();
        assert_eq!(mesh_arrived, 4);
    }

    #[test]
    fn peer_files_lookup_returns_known_peer_files() {
        assert_eq!(peer_files("pine").len(), pine_files().len());
        assert_eq!(peer_files("birch").len(), birch_files().len());
        assert_eq!(peer_files("oak").len(), oak_files().len());
        assert!(peer_files("cedar").is_empty());
        assert!(peer_files("nonexistent").is_empty());
    }
}
