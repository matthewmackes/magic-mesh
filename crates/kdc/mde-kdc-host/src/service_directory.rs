//! KDC-MESH-7 — the mesh-wide **service directory** (design #7).
//!
//! Each node publishes the KDC service set it offers — files, run-commands,
//! placement-local Workloads lifecycle, media, battery, telephony,
//! find-my-device — plus a
//! shallow snapshot of its **shared roots** to a replicated substrate directory
//! (`<root>/kdc-services/<host>.json`, own-row authority, the same mesh-shunt
//! pattern the phone roster + notification relay ride). The phone (via the mesh
//! endpoint) and the desktop Phones hub read the whole directory
//! ([`collect_all_services`]) and **target any node** over the overlay
//! ([`select_node`]) — the "pick the node" any-node reach lock (#7).
//!
//! Because the shared-roots snapshot rides the substrate, a phone/hub can browse
//! every node's shared files' top level from the directory alone (mesh-native, no
//! per-node live connection); a deeper live browse is served by the owning node
//! ([`crate::file_browse`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::file_browse::{FileEntry, SharedRoot};

/// The replicated directory holding every node's published service set.
#[must_use]
pub fn services_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("kdc-services")
}

/// The stable service tokens a node may advertise.
///
/// These are `&'static str` constants rather than an enum, keeping the
/// published set open and forward-compatible. A newer node can advertise a
/// token an older reader does not know; it is ignored rather than rejected
/// during parsing (§6 forward-compat).
pub mod service {
    /// Two-way file transfer + the shared-roots browse (design #11).
    pub const FILES: &str = "files";
    /// Curated run-commands (design #12).
    pub const RUN_COMMANDS: &str = "run-commands";
    /// Construct Workloads lifecycle on the selected node (design #12).
    pub const WORKLOADS: &str = "workloads";
    /// Battery + connectivity report (design #12).
    pub const BATTERY: &str = "battery";
    /// Telephony (call/SMS) alerts (design #12).
    pub const TELEPHONY: &str = "telephony";
    /// Find-my-device — ring this node (design #12).
    pub const FIND_MY_DEVICE: &str = "find-my-device";
    /// Browse the phone's filesystem from this node via SFTP (design #11a).
    pub const SFTP: &str = "sftp";
}

/// A node's shared root as published in the directory.
///
/// This contains the browseable root and a shallow snapshot of its top-level
/// entries, so a phone/hub browses the first level straight off the substrate.
/// A deeper browse reaches the owning node.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedRoot {
    /// The browseable root (label + absolute path).
    #[serde(flatten)]
    pub root: SharedRoot,
    /// The root's top-level entries at publish time (shallow — one level).
    #[serde(default)]
    pub entries: Vec<FileEntry>,
}

/// One node's published service set (the JSON shape on the volume).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeServices {
    /// The publishing node's hostname (the directory key + display name).
    pub node_host: String,
    /// The publishing node's KDC device id (`/etc/machine-id`) — the key its
    /// overlay IP lands under in the transport peer directory (KDC-MESH-2), so a
    /// browse/run-command can be dialed to it over the overlay.
    #[serde(default)]
    pub node_device_id: String,
    /// The node's Nebula overlay IP, when known. `None` until enroll records it
    /// (an honest gate — not dialable yet, §7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_ip: Option<String>,
    /// The service tokens this node offers ([`service`]).
    #[serde(default)]
    pub services: Vec<String>,
    /// The node's shared roots + their shallow snapshot (design #11b).
    #[serde(default)]
    pub shared_roots: Vec<PublishedRoot>,
    /// Unix-ms the node last published (freshness; the reader may age it out).
    #[serde(default)]
    pub updated_ms: i64,
}

impl NodeServices {
    /// Whether this node advertises `svc` (one of the [`service`] tokens).
    #[must_use]
    pub fn offers(&self, svc: &str) -> bool {
        self.services.iter().any(|s| s == svc)
    }

    /// The node's shared roots as plain [`SharedRoot`]s (for a live browse).
    #[must_use]
    pub fn roots(&self) -> Vec<SharedRoot> {
        self.shared_roots.iter().map(|p| p.root.clone()).collect()
    }
}

/// Write this node's service set to its own directory file (atomic temp +
/// rename, own-row authority — only this box writes `<host>.json`).
///
/// # Errors
/// IO / serialization failures.
pub fn publish_services(
    workgroup_root: &Path,
    services: &NodeServices,
) -> std::io::Result<PathBuf> {
    let dir = services_dir(workgroup_root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", services.node_host));
    let body = serde_json::to_string_pretty(services)?;
    let tmp = dir.join(format!(".{}.json.tmp", services.node_host));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Keep replicated service-directory rows bounded before `serde_json` can
/// materialize attacker-controlled collections and strings.
const MAX_SERVICE_DIRECTORY_BYTES: usize = 256 * 1024;

/// Read one replicated service-directory row through a descriptor-backed
/// regular-file boundary.
///
/// The final path component is opened without following a symlink and with
/// non-blocking semantics, so a FIFO or other special file cannot stall the
/// directory reader before the descriptor is inspected and rejected. The
/// second metadata check rejects anything that is not a regular file. Reading
/// one byte beyond the initial metadata bound closes the race where a peer
/// grows a regular file while it is being consumed. Invalid UTF-8 is rejected
/// before any JSON materialization. The caller intentionally folds every
/// failure to its existing fail-soft behavior.
fn read_bounded_service_directory_row(path: &Path) -> Option<String> {
    use std::io::Read as _;

    let mut options = std::fs::OpenOptions::new();
    options.read(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        #[cfg(any(target_os = "linux", target_os = "android"))]
        options.custom_flags(0o400_000 | 0o4000 | 0o2_000_000); // O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        options.custom_flags(0x100 | 0x4); // O_NOFOLLOW | O_NONBLOCK

        // Unsupported Unix targets still fail closed for a final symlink even
        // when their standard library does not expose O_NOFOLLOW.
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )))]
        if !std::fs::symlink_metadata(path)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_file())
        {
            return None;
        }
    }

    #[cfg(not(unix))]
    if !std::fs::symlink_metadata(path)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_file())
    {
        return None;
    }

    let file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_SERVICE_DIRECTORY_BYTES as u64 {
        return None;
    }

    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(MAX_SERVICE_DIRECTORY_BYTES)
            .min(MAX_SERVICE_DIRECTORY_BYTES)
            .saturating_add(1),
    );
    file.take((MAX_SERVICE_DIRECTORY_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_SERVICE_DIRECTORY_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Read every node's published service set (own row and neighbors).
///
/// This is the whole mesh directory a phone/hub browses to pick a node. Junk or
/// half-replicated files are skipped, like every other replicated reader. Rows
/// are sorted by hostname for a deterministic listing.
#[must_use]
pub fn collect_all_services(workgroup_root: &Path) -> Vec<NodeServices> {
    let Ok(entries) = std::fs::read_dir(services_dir(workgroup_root)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Some(raw) = read_bounded_service_directory_row(&path) else {
            continue;
        };
        if let Ok(node) = serde_json::from_str::<NodeServices>(&raw) {
            out.push(node);
        }
    }
    out.sort_by(|a, b| a.node_host.cmp(&b.node_host));
    out
}

/// Select one node from the directory by hostname or KDC device id.
///
/// This is the "pick the node" step (#7). Returns `None` when no such node is
/// published (an honest miss because the node has not synced its directory row).
#[must_use]
pub fn select_node<'a>(all: &'a [NodeServices], node: &str) -> Option<&'a NodeServices> {
    all.iter()
        .find(|n| n.node_host == node || n.node_device_id == node)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(host: &str, id: &str, services: &[&str]) -> NodeServices {
        NodeServices {
            node_host: host.into(),
            node_device_id: id.into(),
            overlay_ip: Some("10.42.0.9".into()),
            services: services.iter().map(|s| (*s).to_string()).collect(),
            shared_roots: vec![PublishedRoot {
                root: SharedRoot::new("Public", "/home/mm/Public"),
                entries: vec![FileEntry {
                    name: "a.txt".into(),
                    is_dir: false,
                    size: 5,
                }],
            }],
            updated_ms: 1,
        }
    }

    #[test]
    fn publish_then_collect_round_trips_every_node() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        publish_services(
            root,
            &node("nodeA", "id-a", &[service::FILES, service::WORKLOADS]),
        )
        .unwrap();
        publish_services(root, &node("nodeB", "id-b", &[service::FILES])).unwrap();

        let all = collect_all_services(root);
        assert_eq!(all.len(), 2);
        // Sorted by hostname.
        assert_eq!(all[0].node_host, "nodeA");
        assert_eq!(all[1].node_host, "nodeB");
        // The shared-roots snapshot rode the substrate — browsable off the directory.
        assert_eq!(all[0].shared_roots[0].entries[0].name, "a.txt");
    }

    #[test]
    fn select_node_finds_by_host_or_device_id() {
        let all = vec![node("nodeA", "id-a", &[service::FILES])];
        assert_eq!(select_node(&all, "nodeA").unwrap().node_host, "nodeA");
        assert_eq!(select_node(&all, "id-a").unwrap().node_host, "nodeA");
        assert!(select_node(&all, "ghost").is_none());
    }

    #[test]
    fn offers_reflects_the_published_service_set() {
        let n = node("nodeA", "id-a", &[service::FILES, service::WORKLOADS]);
        assert!(n.offers(service::FILES));
        assert!(n.offers(service::WORKLOADS));
        assert!(!n.offers(service::TELEPHONY));
    }

    #[test]
    fn a_republish_overwrites_the_nodes_own_row() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        publish_services(root, &node("nodeA", "id-a", &[service::FILES])).unwrap();
        publish_services(
            root,
            &node("nodeA", "id-a", &[service::FILES, service::TELEPHONY]),
        )
        .unwrap();
        let all = collect_all_services(root);
        assert_eq!(all.len(), 1, "own row overwritten, not duplicated");
        assert!(all[0].offers(service::TELEPHONY));
    }

    #[test]
    fn junk_files_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(services_dir(root)).unwrap();
        std::fs::write(services_dir(root).join("bad.json"), b"not json").unwrap();
        publish_services(root, &node("nodeA", "id-a", &[service::FILES])).unwrap();
        let all = collect_all_services(root);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].node_host, "nodeA");
    }

    #[test]
    fn bounded_reader_preserves_a_valid_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let services = node("nodeA", "id-a", &[service::FILES]);

        publish_services(root, &services).unwrap();

        let path = services_dir(root).join("nodeA.json");
        let raw = read_bounded_service_directory_row(&path).expect("valid row is readable");
        assert_eq!(
            serde_json::from_str::<NodeServices>(&raw).unwrap(),
            services
        );
    }

    #[cfg(unix)]
    #[test]
    fn directory_reader_skips_hostile_leaves_without_following_or_blocking() {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dir = services_dir(root);
        std::fs::create_dir_all(&dir).unwrap();

        let valid_a = node("nodeA", "id-a", &[service::FILES]);
        let valid_b = node("nodeB", "id-b", &[service::WORKLOADS]);
        publish_services(root, &valid_b).unwrap();
        publish_services(root, &valid_a).unwrap();

        let outside = root.join("outside.json");
        let outside_body =
            serde_json::to_vec(&node("outside", "id-outside", &[service::FILES])).unwrap();
        std::fs::write(&outside, &outside_body).unwrap();
        symlink(&outside, dir.join("symlink.json")).unwrap();
        std::fs::write(
            dir.join("oversized.json"),
            vec![b'x'; MAX_SERVICE_DIRECTORY_BYTES + 1],
        )
        .unwrap();
        std::fs::write(dir.join("invalid-utf8.json"), [0xff, 0xfe]).unwrap();
        std::fs::create_dir(dir.join("directory.json")).unwrap();
        let _socket = UnixListener::bind(dir.join("special.json")).unwrap();

        assert!(read_bounded_service_directory_row(&dir.join("symlink.json")).is_none());
        assert!(read_bounded_service_directory_row(&dir.join("oversized.json")).is_none());
        assert!(read_bounded_service_directory_row(&dir.join("invalid-utf8.json")).is_none());
        assert!(read_bounded_service_directory_row(&dir.join("directory.json")).is_none());
        assert!(read_bounded_service_directory_row(&dir.join("special.json")).is_none());

        let all = collect_all_services(root);
        assert_eq!(
            all.iter()
                .map(|node| node.node_host.as_str())
                .collect::<Vec<_>>(),
            vec!["nodeA", "nodeB"]
        );
    }

    #[test]
    fn an_unknown_service_token_is_ignored_not_a_parse_error() {
        // Forward-compat: a newer node advertising a token this reader doesn't
        // know still parses; the token is simply carried through.
        let raw = r#"{"node_host":"nodeZ","services":["files","warp-drive"]}"#;
        let n: NodeServices = serde_json::from_str(raw).unwrap();
        assert!(n.offers(service::FILES));
        assert!(n.offers("warp-drive"));
    }
}
