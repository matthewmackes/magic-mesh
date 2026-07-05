//! KDC-MESH-7 sftp plugin — `kdeconnect.sftp` + `kdeconnect.sftp.request`.
//!
//! KDE Connect's SFTP plugin lets a desktop **browse the phone's filesystem**:
//! the desktop sends `kdeconnect.sftp.request` with `startBrowsing = true`; the
//! phone stands up an on-device SFTP server and replies with a `kdeconnect.sftp`
//! packet carrying the connection parameters (`ip`/`port`/`user`/`password` +
//! the exported paths). The desktop then mounts that server (production: `sshfs`)
//! and the phone's files appear as a local directory.
//!
//! We control only the host/desktop side (the phone runs the stock KDE Connect
//! Android app), so this module models the two wire bodies + their builders; the
//! actual mount is the injectable seam in `mde_kdc_host::sftp` (KDC-MESH-7 #11a).

use serde::{Deserialize, Serialize};

use crate::wire::Packet;

/// `kdeconnect.sftp.request` body — the desktop's request to start browsing.
///
/// Upstream sends `{"startBrowsing": true}`; the phone answers with a
/// [`SftpMountInfo`] `kdeconnect.sftp` packet.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpRequestBody {
    /// Ask the phone to stand up its SFTP server + reply with the mount info.
    #[serde(default)]
    pub start_browsing: bool,
}

/// `kdeconnect.sftp` body — the phone's reply carrying the SFTP server's
/// connection parameters.
///
/// The phone runs the SFTP server; the desktop is the client. `multi_paths` +
/// `path_names` (present on newer Android clients) expose several named exports
/// (Internal storage, SD card, …) under one server; older clients export a
/// single `path`. Fields absent on the wire default cleanly so a minimal reply
/// still parses (§7 — never guessed).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpMountInfo {
    /// The phone's address the desktop dials the SFTP server at. Over the mesh
    /// this is the phone's Nebula overlay IP (KDC-MESH-1/2); on stock LAN it's
    /// the phone's LAN IP.
    #[serde(default)]
    pub ip: String,
    /// The SFTP server's TCP port (ephemeral — the phone picks it per session).
    #[serde(default)]
    pub port: u16,
    /// The SFTP username the phone minted for this session.
    #[serde(default)]
    pub user: String,
    /// The one-time SFTP password the phone minted for this session.
    #[serde(default)]
    pub password: String,
    /// The single exported path (older Android clients). Empty when the client
    /// uses `multi_paths` instead.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// The exported remote paths (newer Android clients export several).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub multi_paths: Vec<String>,
    /// Friendly names paired 1:1 with [`multi_paths`] (Internal storage, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_names: Vec<String>,
}

impl SftpMountInfo {
    /// Whether the reply carries enough to actually dial + mount the server: an
    /// address, a port, and credentials. A reply missing any of these is an
    /// honest no-mount (the phone declined / SFTP is off), not a faked mount.
    #[must_use]
    pub fn is_mountable(&self) -> bool {
        !self.ip.is_empty() && self.port != 0 && !self.user.is_empty() && !self.password.is_empty()
    }

    /// The remote path to mount: the first `multi_paths` entry when present, else
    /// the single `path`, else the server root `/`. Newer clients expose several
    /// exports; mounting the first (Internal storage, conventionally) is the
    /// upstream default browse target.
    #[must_use]
    pub fn remote_path(&self) -> &str {
        if let Some(first) = self.multi_paths.first() {
            first
        } else if !self.path.is_empty() {
            &self.path
        } else {
            "/"
        }
    }
}

/// Build a `kdeconnect.sftp.request` packet asking the phone to start browsing.
#[must_use]
pub fn sftp_request_packet(id_ms: i64, start_browsing: bool) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.sftp.request".to_string(),
        body: serde_json::to_value(SftpRequestBody { start_browsing })
            .expect("SftpRequestBody is always JSON-serializable"),
        mde_caps: None,
        payload_size: None,
        payload_transfer_info: None,
    }
}

/// Build a `kdeconnect.sftp` mount-info packet (the phone's reply shape — used by
/// tests + any host-side SFTP responder).
#[must_use]
pub fn sftp_mount_packet(id_ms: i64, info: SftpMountInfo) -> Packet {
    Packet {
        id: id_ms,
        kind: "kdeconnect.sftp".to_string(),
        body: serde_json::to_value(info).expect("SftpMountInfo is always JSON-serializable"),
        mde_caps: None,
        payload_size: None,
        payload_transfer_info: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{from_packet_body, PluginKind};

    #[test]
    fn sftp_request_packet_kind_and_body() {
        let p = sftp_request_packet(1, true);
        assert_eq!(p.kind, "kdeconnect.sftp.request");
        let body: SftpRequestBody = from_packet_body(&p).unwrap();
        assert!(body.start_browsing);
    }

    #[test]
    fn sftp_request_packet_kind_matches_plugin_token() {
        let p = sftp_request_packet(1, true);
        assert_eq!(p.kind, PluginKind::Sftp.packet_kind());
    }

    #[test]
    fn sftp_mount_info_round_trips_via_wire() {
        let info = SftpMountInfo {
            ip: "10.42.0.9".into(),
            port: 1739,
            user: "kdeconnect".into(),
            password: "secret".into(),
            path: String::new(),
            multi_paths: vec!["/storage/emulated/0".into(), "/storage/sdcard".into()],
            path_names: vec!["Internal storage".into(), "SD card".into()],
        };
        let p = sftp_mount_packet(7, info.clone());
        assert_eq!(p.kind, "kdeconnect.sftp");
        let wire = serde_json::to_string(&p).unwrap();
        let decoded: Packet = serde_json::from_str(&wire).unwrap();
        let back: SftpMountInfo = from_packet_body(&decoded).unwrap();
        assert_eq!(back, info);
    }

    #[test]
    fn is_mountable_requires_addr_port_and_creds() {
        let full = SftpMountInfo {
            ip: "10.42.0.9".into(),
            port: 1739,
            user: "u".into(),
            password: "p".into(),
            ..Default::default()
        };
        assert!(full.is_mountable());
        // Any missing field ⇒ not mountable (honest no-mount, not a faked one).
        assert!(!SftpMountInfo {
            ip: String::new(),
            ..full.clone()
        }
        .is_mountable());
        assert!(!SftpMountInfo {
            port: 0,
            ..full.clone()
        }
        .is_mountable());
        assert!(!SftpMountInfo {
            user: String::new(),
            ..full.clone()
        }
        .is_mountable());
        assert!(!SftpMountInfo {
            password: String::new(),
            ..full
        }
        .is_mountable());
    }

    #[test]
    fn remote_path_prefers_multi_then_single_then_root() {
        let multi = SftpMountInfo {
            multi_paths: vec!["/a".into(), "/b".into()],
            path: "/single".into(),
            ..Default::default()
        };
        assert_eq!(multi.remote_path(), "/a");
        let single = SftpMountInfo {
            path: "/single".into(),
            ..Default::default()
        };
        assert_eq!(single.remote_path(), "/single");
        let empty = SftpMountInfo::default();
        assert_eq!(empty.remote_path(), "/");
    }

    #[test]
    fn minimal_reply_still_parses() {
        // A phone that declines browsing may reply with a near-empty body; it must
        // parse (and read as not-mountable) rather than error.
        let raw = r#"{"kind":"x"}"#;
        let info: SftpMountInfo = serde_json::from_str(raw).unwrap();
        assert!(!info.is_mountable());
    }
}
