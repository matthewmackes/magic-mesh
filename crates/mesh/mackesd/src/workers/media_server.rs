//! MEDIA-15 — the mackesd **mesh media server + DLNA/UPnP + aggregation**.
//!
//! Design: `docs/design/mesh-media-player.md` (rows 27 "Mesh library" + 30
//! "Server role"). The producer half of the mesh media plane MEDIA-14
//! (`media_sources.rs`) discovers and MEDIA-8 (`mde-media-egui`) renders. Three
//! duties, all §6 mesh-side glue over the existing replicated plane:
//!
//! 1. **Share this node's chosen folders as a mesh media source.** Scan the
//!    chosen shared folders (config/env, [`resolve_share_dirs`]) into a
//!    [`ShareManifest`], and write it to the SAME replicated QNM-Shared plane
//!    the other published services ride (`<mount>/<host>/media-library.json`,
//!    alongside `media-registry.json` / `compute-inventory.json`) so every
//!    peer's aggregator reads it. This node is **advertised** to peers' MEDIA-14
//!    by binding the mesh HTTP media server on the pinned
//!    [`MESH_MEDIA_PORT`], which the localhost descriptor probe
//!    (`descriptors::MEDIA_PORTS`, the `mde-media`/[`media_sources::SERVICE_MESH_PLAYER`]
//!    row) then folds into this peer's `descriptors.media` — no second
//!    advertisement channel is minted.
//!
//! 2. **A DLNA/UPnP media server** exposing the shared folders on the LAN. The
//!    UPnP device description + the ContentDirectory DIDL-Lite browse response
//!    are built from the manifest ([`upnp_device_description`] /
//!    [`didl_lite_from_manifest`], pure + tested) and served over the same mesh
//!    HTTP server. The **live-network leg** — the SSDP multicast announce a LAN
//!    TV discovers the server by ([`ssdp_alive_notify`]) — is **honestly gated**
//!    (mirrors `mesh_mount`): a container/headless box with no multicast route
//!    surfaces a `gated:` note rather than faking discovery; the manifest +
//!    served DLNA XML are real regardless.
//!
//! 3. **Aggregate peers' shared media into ONE mesh library view.** Read every
//!    peer's `media-library.json` off the plane ([`read_manifests`]) and fold
//!    them into one deduped, per-node-attributed [`MeshLibrary`]
//!    ([`merge_libraries`], the load-bearing merge the acceptance pins), then
//!    publish it to [`MESH_LIBRARY_TOPIC`] (`state/media/library`) for the
//!    MEDIA-8 Library panel. Dedup is by content id (title + size), so the same
//!    file shared from two nodes collapses to one entry attributed to both.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};

/// The retained-latest state topic the aggregated mesh library is published to.
/// The MEDIA-8 Library panel reads the newest record off this topic.
pub const MESH_LIBRARY_TOPIC: &str = "state/media/library";

/// File name a node writes its share manifest to under its QNM-Shared dir — the
/// SAME replicated plane `media-registry.json` / `compute-inventory.json` ride.
/// Every node reads these to aggregate the whole mesh library.
pub const MESH_LIBRARY_MANIFEST_FILE: &str = "media-library.json";

/// The pinned mesh HTTP media-server port. Binding it is what makes the
/// localhost descriptor probe (`descriptors::MEDIA_PORTS`) advertise this node's
/// [`SERVICE_MESH_PLAYER`] (`mde-media`) service, so peers' MEDIA-14 discovery
/// finds it. Pinned here + in `descriptors::MEDIA_PORTS` so the producer and the
/// scan agree byte-for-byte (matches the `media_sources` mesh-player test port).
pub const MESH_MEDIA_PORT: u16 = 9600;

/// The env override for the chosen shared folders (`:`-separated absolute
/// paths). The MEDIA-8 surface writes them to [`share_config_path`]; the env
/// var is the deploy/systemd-unit override.
pub const SHARE_DIRS_ENV: &str = "MDE_MEDIA_SHARE_DIRS";

/// Rescan + republish cadence. Media folders change at human pace; a 30 s tick
/// keeps the library fresh without hammering the filesystem.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Unconditional-republish heartbeat: between heartbeats the library publishes
/// only on change; once elapsed it republishes so a late subscriber / pruned
/// topic still finds a recent record (mirrors `media_sources`).
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(300);

/// Bound the shared-folder walk depth — a shared media folder is not a
/// filesystem root; 8 levels is generous and stops a symlink/loop from spinning.
const MAX_SCAN_DEPTH: usize = 8;

/// Bound the manifest item count — a runaway share never balloons the manifest
/// past a size the plane + Bus can carry.
const MAX_ITEMS_PER_MANIFEST: usize = 20_000;

/// The SSDP multicast group + port a LAN UPnP control point listens on.
const SSDP_MULTICAST: &str = "239.255.255.250:1900";

// ───────────────────────────── data model ─────────────────────────────

/// The kind of a shared media item, classified by file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaItemKind {
    /// A video file (mp4/mkv/…).
    Video,
    /// An audio file (mp3/flac/…).
    Audio,
    /// An image file (jpg/png/…).
    Image,
}

impl MediaItemKind {
    /// The UPnP `upnp:class` for a DIDL-Lite `<item>` of this kind.
    #[must_use]
    pub const fn upnp_class(self) -> &'static str {
        match self {
            Self::Video => "object.item.videoItem",
            Self::Audio => "object.item.audioItem.musicTrack",
            Self::Image => "object.item.imageItem.photo",
        }
    }
}

/// Classify a lowercased file extension into a media kind (`None` when it isn't
/// a media file — those are honestly skipped, never shared).
#[must_use]
pub fn media_kind_from_ext(ext: &str) -> Option<MediaItemKind> {
    match ext
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg" | "ts"
        | "m2ts" | "ogv" => Some(MediaItemKind::Video),
        "mp3" | "flac" | "aac" | "ogg" | "opus" | "wav" | "m4a" | "wma" | "alac" => {
            Some(MediaItemKind::Audio)
        }
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "tiff" | "tif" | "heic" => {
            Some(MediaItemKind::Image)
        }
        _ => None,
    }
}

/// A best-effort HTTP `Content-Type` / DLNA `protocolInfo` MIME for a kind+ext.
#[must_use]
pub fn mime_for(kind: MediaItemKind, ext: &str) -> String {
    let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    let specific = match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" | "ogv" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "m4a" | "aac" | "alac" => "audio/mp4",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "",
    };
    if specific.is_empty() {
        match kind {
            MediaItemKind::Video => "video/octet-stream".to_string(),
            MediaItemKind::Audio => "audio/octet-stream".to_string(),
            MediaItemKind::Image => "image/octet-stream".to_string(),
        }
    } else {
        specific.to_string()
    }
}

/// One shared media file — a row of this node's [`ShareManifest`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MediaItem {
    /// Stable content id (`hex(sha256(title\0size))[..16]`). Two identical files
    /// (same title + size) on two nodes share an id, so the aggregation dedups
    /// them into one entry attributed to both nodes.
    pub id: String,
    /// Display title (the file stem).
    pub title: String,
    /// Path relative to its shared-folder root (the served locator + display).
    pub rel_path: String,
    /// The media kind (video/audio/image).
    pub kind: MediaItemKind,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Last-modified wall-clock (ms since the Unix epoch; 0 when unavailable).
    pub mtime_ms: u64,
}

/// A stable content id for a media item — derived from title + size so the same
/// file shared by two nodes collapses to one aggregated entry.
#[must_use]
pub fn item_id(title: &str, size_bytes: u64) -> String {
    let mut h = Sha256::new();
    h.update(title.as_bytes());
    h.update([0u8]);
    h.update(size_bytes.to_le_bytes());
    let digest = h.finalize();
    let mut s = String::with_capacity(16);
    for b in digest.iter().take(8) {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// This node's shared-media manifest — written to the replicated plane so peers
/// aggregate it into the mesh library.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShareManifest {
    /// Publishing node id.
    pub node: String,
    /// Publishing hostname (the plane key + the mesh media host clients dial).
    pub host: String,
    /// The mesh HTTP media-server port ([`MESH_MEDIA_PORT`]).
    pub port: u16,
    /// The shared-folder labels this manifest was scanned from.
    pub folders: Vec<String>,
    /// The shared media items, deduped + sorted.
    pub items: Vec<MediaItem>,
    /// Wall-clock publish time (ms since the Unix epoch).
    pub published_at_ms: u64,
}

/// One aggregated mesh-library entry — a media item plus the nodes that serve
/// it (per-node attribution, the acceptance's merge requirement).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MeshLibraryEntry {
    /// The deduped media item (the first-seen row's fields).
    pub item: MediaItem,
    /// The hostnames that share this item, deduped + sorted.
    pub nodes: Vec<String>,
}

/// The aggregated mesh library — every peer's shared media folded into one
/// deduped, per-node-attributed view.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MeshLibrary {
    /// The deduped, sorted entries.
    pub entries: Vec<MeshLibraryEntry>,
    /// Distinct nodes contributing to the library.
    pub node_count: usize,
    /// Total deduped items.
    pub item_count: usize,
}

/// This node's own serving status — so the MEDIA-8 surface can say whether this
/// node is a live mesh media source (and honestly why the DLNA leg is gated).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServerStatus {
    /// The mesh HTTP media-server port.
    pub port: u16,
    /// The shared-folder labels this node serves.
    pub shared_folders: Vec<String>,
    /// Items this node shares.
    pub shared_item_count: usize,
    /// Mesh HTTP media-server status (`ok …` / `gated: …`).
    pub http: String,
    /// DLNA/UPnP SSDP announce status (`ok …` / `gated: …` — the honest live
    /// leg).
    pub ssdp: String,
}

/// The full record published to [`MESH_LIBRARY_TOPIC`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MeshLibraryState {
    /// Publishing node id.
    pub node: String,
    /// This node's own serving status.
    pub server: ServerStatus,
    /// The aggregated mesh library (this node + every peer).
    pub library: MeshLibrary,
    /// Wall-clock publish time (ms since the Unix epoch).
    pub published_at_ms: u64,
}

// ───────────────────────── the chosen shared folders ─────────────────────────

/// Resolve the chosen shared folders. Precedence, all honest:
///
/// 1. `env_val` (the [`SHARE_DIRS_ENV`] override, `:`-separated), else
/// 2. `config_body` — the MEDIA-8-written `{"folders":[…]}` config, else
/// 3. the standard media dirs under `home` (`Videos` / `Music` / `Pictures`).
///
/// Pure — existence is NOT filtered here (deterministic for tests); the scan
/// skips a folder that doesn't exist, so a stale config never fabricates items.
#[must_use]
pub fn resolve_share_dirs(
    env_val: Option<&str>,
    config_body: Option<&str>,
    home: &Path,
) -> Vec<PathBuf> {
    if let Some(raw) = env_val {
        let dirs: Vec<PathBuf> = raw
            .split(':')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
        if !dirs.is_empty() {
            return dirs;
        }
    }
    if let Some(body) = config_body {
        if let Ok(cfg) = serde_json::from_str::<ShareConfig>(body) {
            let dirs: Vec<PathBuf> = cfg
                .folders
                .into_iter()
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from)
                .collect();
            if !dirs.is_empty() {
                return dirs;
            }
        }
    }
    ["Videos", "Music", "Pictures"]
        .iter()
        .map(|d| home.join(d))
        .collect()
}

/// The MEDIA-8-written chosen-folders config (`{"folders":[…]}`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShareConfig {
    /// Absolute paths of the folders to share on the mesh.
    #[serde(default)]
    pub folders: Vec<String>,
}

/// The config path the MEDIA-8 surface writes chosen folders to:
/// `<config>/mde/media/shared-folders.json`.
#[must_use]
pub fn share_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("mde").join("media").join("shared-folders.json"))
}

/// Load the chosen shared folders from the env + config file (the production
/// wrapper around [`resolve_share_dirs`]).
#[must_use]
pub fn load_share_dirs() -> Vec<PathBuf> {
    let env_val = std::env::var(SHARE_DIRS_ENV).ok();
    let config_body = share_config_path().and_then(|p| std::fs::read_to_string(p).ok());
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
    resolve_share_dirs(env_val.as_deref(), config_body.as_deref(), &home)
}

// ───────────────────────────── the manifest scan ─────────────────────────────

/// Scan one shared folder into media items, keyed rel-to-root. Bounded in depth
/// + count; unreadable entries are skipped (best-effort, never fatal). Also
/// records each item's absolute path into `abs` (id → path) for the HTTP server.
fn scan_dir_into(
    root: &Path,
    dir: &Path,
    depth: usize,
    out: &mut Vec<MediaItem>,
    abs: &mut BTreeMap<String, PathBuf>,
) {
    if depth > MAX_SCAN_DEPTH || out.len() >= MAX_ITEMS_PER_MANIFEST {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    names.sort();
    for path in names {
        if out.len() >= MAX_ITEMS_PER_MANIFEST {
            return;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            scan_dir_into(root, &path, depth + 1, out, abs);
            continue;
        }
        if !meta.is_file() {
            continue; // skip symlinks/fifos/sockets — no loop, no surprise serve
        }
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let Some(kind) = media_kind_from_ext(ext) else {
            continue;
        };
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel_path = rel.to_string_lossy().into_owned();
        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| rel_path.clone());
        let size_bytes = meta.len();
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        let id = item_id(&title, size_bytes);
        abs.entry(id.clone()).or_insert_with(|| path.clone());
        out.push(MediaItem {
            id,
            title,
            rel_path,
            kind,
            size_bytes,
            mtime_ms,
        });
    }
}

/// Build this node's share manifest by scanning the chosen folders. Returns the
/// manifest plus the id→absolute-path map the HTTP media server serves bytes
/// from. A folder that doesn't exist is skipped (not listed in `folders`), so a
/// stale config never fabricates a share. Items are deduped by id + sorted by
/// `(title, id)`.
#[must_use]
pub fn build_manifest(
    node: &str,
    host: &str,
    port: u16,
    share_dirs: &[PathBuf],
) -> (ShareManifest, BTreeMap<String, PathBuf>) {
    let mut items = Vec::new();
    let mut abs = BTreeMap::new();
    let mut folders = Vec::new();
    for dir in share_dirs {
        if !dir.is_dir() {
            continue;
        }
        folders.push(dir.to_string_lossy().into_owned());
        scan_dir_into(dir, dir, 0, &mut items, &mut abs);
    }
    items.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    items.dedup_by(|a, b| a.id == b.id);
    let manifest = ShareManifest {
        node: node.to_string(),
        host: host.to_string(),
        port,
        folders,
        items,
        published_at_ms: now_ms(),
    };
    (manifest, abs)
}

// ───────────────────────── the replicated plane I/O ─────────────────────────

/// Write this node's manifest to the replicated QNM-Shared plane at
/// `<mount>/<host>/media-library.json` (atomic tmp+rename). Best-effort — a
/// missing mount / write error is logged, never fatal (mirrors
/// `media_registry::write_shared_registration`).
pub fn write_manifest(mount: &Path, host: &str, manifest: &ShareManifest) {
    if host.is_empty() {
        return;
    }
    let dir = mount.join(host);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(target: "mackesd::media_server", "mkdir {} failed: {e}", dir.display());
        return;
    }
    let Ok(body) = serde_json::to_string(manifest) else {
        return;
    };
    let tmp = dir.join(".media-library.json.tmp");
    let final_path = dir.join(MESH_LIBRARY_MANIFEST_FILE);
    if let Err(e) = std::fs::write(&tmp, body.as_bytes()) {
        tracing::warn!(target: "mackesd::media_server", "write {} failed: {e}", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        tracing::warn!(target: "mackesd::media_server", "rename manifest failed: {e}");
    }
}

/// Read every peer's share manifest off the replicated plane
/// (`<mount>/<host>/media-library.json`). Malformed/unreadable files are
/// skipped (a half-written file from a concurrent writer must not break a
/// reader); a missing mount yields an empty list. The same `read_dir`-over-the-
/// share discipline `mesh_media::read_shared_account_from_plane` uses.
#[must_use]
pub fn read_manifests(mount: &Path) -> Vec<ShareManifest> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(mount) else {
        return out;
    };
    for ent in entries.flatten() {
        let path = ent.path().join(MESH_LIBRARY_MANIFEST_FILE);
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(m) = serde_json::from_str::<ShareManifest>(&body) {
            out.push(m);
        }
    }
    out.sort_by(|a, b| a.host.cmp(&b.host));
    out
}

// ───────────────────────────── the merge fold ─────────────────────────────

/// Fold every peer's share manifest into ONE deduped, per-node-attributed mesh
/// library — the load-bearing merge the acceptance pins. Rules:
///
/// 1. **Union** — every item from every manifest is considered.
/// 2. **Dedup by content id** — the same file (title + size) shared from two
///    nodes collapses to ONE entry (the first-seen row's fields).
/// 3. **Per-node attribution** — each entry carries the deduped, sorted set of
///    hostnames that share it.
///
/// Output is sorted `(title, id)` case-insensitively so the published library is
/// stable across ticks.
#[must_use]
pub fn merge_libraries(manifests: &[ShareManifest]) -> MeshLibrary {
    let mut by_id: BTreeMap<String, MeshLibraryEntry> = BTreeMap::new();
    let mut nodes: BTreeSet<String> = BTreeSet::new();
    for m in manifests {
        let node = if m.host.is_empty() {
            m.node.clone()
        } else {
            m.host.clone()
        };
        nodes.insert(node.clone());
        for item in &m.items {
            let entry = by_id
                .entry(item.id.clone())
                .or_insert_with(|| MeshLibraryEntry {
                    item: item.clone(),
                    nodes: Vec::new(),
                });
            if !entry.nodes.iter().any(|n| n == &node) {
                entry.nodes.push(node.clone());
            }
        }
    }
    let mut entries: Vec<MeshLibraryEntry> = by_id.into_values().collect();
    for e in &mut entries {
        e.nodes.sort();
        e.nodes.dedup();
    }
    entries.sort_by(|a, b| {
        a.item
            .title
            .to_lowercase()
            .cmp(&b.item.title.to_lowercase())
            .then_with(|| a.item.id.cmp(&b.item.id))
    });
    let item_count = entries.len();
    MeshLibrary {
        entries,
        node_count: nodes.len(),
        item_count,
    }
}

// ───────────────────────── DLNA/UPnP — pure XML builders ─────────────────────────

/// A stable UPnP UDN (uuid) for this host — derived from the hostname so a
/// device keeps its identity across restarts without persisting a file.
#[must_use]
pub fn stable_uuid(host: &str) -> String {
    let digest = Sha256::digest(host.as_bytes());
    let mut hex = String::with_capacity(32);
    for b in digest.iter().take(16) {
        let _ = write!(hex, "{b:02x}");
    }
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// XML-escape a text value for a DIDL-Lite / device-description body.
#[must_use]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Build the UPnP root device description a control point fetches at
/// `LOCATION` (`/description.xml`). Declares a `MediaServer:1` device carrying a
/// `ContentDirectory:1` + `ConnectionManager:1` service — the standard DLNA
/// media-server shape.
#[must_use]
pub fn upnp_device_description(uuid: &str, friendly_name: &str, host: &str, port: u16) -> String {
    let base = format!("http://{host}:{port}");
    format!(
        "<?xml version=\"1.0\"?>\n\
<root xmlns=\"urn:schemas-upnp-org:device-1-0\">\n\
  <specVersion><major>1</major><minor>0</minor></specVersion>\n\
  <URLBase>{base}</URLBase>\n\
  <device>\n\
    <deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>\n\
    <friendlyName>{name}</friendlyName>\n\
    <manufacturer>MCNF</manufacturer>\n\
    <modelName>mde-media</modelName>\n\
    <UDN>uuid:{uuid}</UDN>\n\
    <serviceList>\n\
      <service>\n\
        <serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType>\n\
        <serviceId>urn:upnp-org:serviceId:ContentDirectory</serviceId>\n\
        <SCPDURL>/ContentDirectory.xml</SCPDURL>\n\
        <controlURL>/ContentDirectory/control</controlURL>\n\
        <eventSubURL>/ContentDirectory/event</eventSubURL>\n\
      </service>\n\
      <service>\n\
        <serviceType>urn:schemas-upnp-org:service:ConnectionManager:1</serviceType>\n\
        <serviceId>urn:upnp-org:serviceId:ConnectionManager</serviceId>\n\
        <SCPDURL>/ConnectionManager.xml</SCPDURL>\n\
        <controlURL>/ConnectionManager/control</controlURL>\n\
        <eventSubURL>/ConnectionManager/event</eventSubURL>\n\
      </service>\n\
    </serviceList>\n\
  </device>\n\
</root>\n",
        name = xml_escape(friendly_name),
    )
}

/// Build the ContentDirectory DIDL-Lite `Browse` result for this node's
/// manifest — one `<item>` per shared file, each with a `<res>` pointing at the
/// mesh HTTP media server's `/media/<id>` endpoint. This is the DLNA payload a
/// TV renders as a browsable media list.
#[must_use]
pub fn didl_lite_from_manifest(manifest: &ShareManifest, base_url: &str) -> String {
    let mut body = String::from(
        "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" \
xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\">",
    );
    for item in &manifest.items {
        let ext = Path::new(&item.rel_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let mime = mime_for(item.kind, ext);
        let _ = write!(
            body,
            "<item id=\"{id}\" parentID=\"0\" restricted=\"1\">\
<dc:title>{title}</dc:title>\
<upnp:class>{class}</upnp:class>\
<res protocolInfo=\"http-get:*:{mime}:*\" size=\"{size}\">{base}/media/{id}</res>\
</item>",
            id = xml_escape(&item.id),
            title = xml_escape(&item.title),
            class = item.kind.upnp_class(),
            size = item.size_bytes,
            base = xml_escape(base_url),
        );
    }
    body.push_str("</DIDL-Lite>");
    body
}

/// Build the SSDP `NOTIFY … ssdp:alive` advertisement datagram a LAN control
/// point discovers this MediaServer by. Pure — the SEND of it over multicast is
/// the honestly-gated live leg ([`ssdp_announce`]).
#[must_use]
pub fn ssdp_alive_notify(uuid: &str, host: &str, port: u16) -> String {
    format!(
        "NOTIFY * HTTP/1.1\r\n\
HOST: 239.255.255.250:1900\r\n\
CACHE-CONTROL: max-age=1800\r\n\
LOCATION: http://{host}:{port}/description.xml\r\n\
NT: urn:schemas-upnp-org:device:MediaServer:1\r\n\
NTS: ssdp:alive\r\n\
SERVER: MCNF/1.0 UPnP/1.0 mde-media/1.0\r\n\
USN: uuid:{uuid}::urn:schemas-upnp-org:device:MediaServer:1\r\n\
\r\n"
    )
}

/// Attempt the SSDP multicast announce — the honestly-gated DLNA discovery leg.
/// Sends one `ssdp:alive` NOTIFY to the multicast group. Returns an `Err(String)`
/// when the box has no multicast route (container/headless/farm) so the caller
/// surfaces a `gated:` status rather than faking LAN discovery.
///
/// # Errors
/// A UDP socket bind or multicast `send_to` failure (no multicast route).
pub fn ssdp_announce(uuid: &str, host: &str, port: u16) -> Result<(), String> {
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("ssdp bind: {e}"))?;
    let datagram = ssdp_alive_notify(uuid, host, port);
    sock.send_to(datagram.as_bytes(), SSDP_MULTICAST)
        .map_err(|e| format!("ssdp send: {e}"))?;
    Ok(())
}

// ─────────────────────── the mesh HTTP media server (live) ───────────────────────

/// Shared read-only state the HTTP media server serves each request from — the
/// current manifest + the id→absolute-path map, swapped atomically on rescan.
#[derive(Default)]
struct ServeState {
    manifest: ShareManifest,
    abs: BTreeMap<String, PathBuf>,
    uuid: String,
    friendly_name: String,
    host: String,
    port: u16,
}

impl Default for ShareManifest {
    fn default() -> Self {
        Self {
            node: String::new(),
            host: String::new(),
            port: MESH_MEDIA_PORT,
            folders: Vec::new(),
            items: Vec::new(),
            published_at_ms: 0,
        }
    }
}

/// Route one parsed request path to its HTTP response (status line, content
/// type, body bytes). Pure over the served state, so routing is unit-tested
/// without a socket.
fn route_request(state: &ServeState, path: &str) -> (u16, String, Vec<u8>) {
    let path = path.split('?').next().unwrap_or(path);
    match path {
        "/" | "/description.xml" => (
            200,
            "text/xml; charset=\"utf-8\"".to_string(),
            upnp_device_description(&state.uuid, &state.friendly_name, &state.host, state.port)
                .into_bytes(),
        ),
        "/manifest.json" => (
            200,
            "application/json".to_string(),
            serde_json::to_vec(&state.manifest).unwrap_or_default(),
        ),
        "/ContentDirectory/browse" | "/didl.xml" => {
            let base = format!("http://{}:{}", state.host, state.port);
            (
                200,
                "text/xml; charset=\"utf-8\"".to_string(),
                didl_lite_from_manifest(&state.manifest, &base).into_bytes(),
            )
        }
        p if p.starts_with("/media/") => {
            let id = &p["/media/".len()..];
            match state.abs.get(id) {
                Some(abs) => match std::fs::read(abs) {
                    Ok(bytes) => {
                        let item = state.manifest.items.iter().find(|i| i.id == id);
                        let ext = Path::new(abs)
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("");
                        let mime = item.map_or_else(
                            || "application/octet-stream".to_string(),
                            |i| mime_for(i.kind, ext),
                        );
                        (200, mime, bytes)
                    }
                    Err(_) => (404, "text/plain".to_string(), b"not found".to_vec()),
                },
                None => (404, "text/plain".to_string(), b"not found".to_vec()),
            }
        }
        _ => (404, "text/plain".to_string(), b"not found".to_vec()),
    }
}

/// The reason phrase for the small set of statuses the server emits.
const fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Error",
    }
}

/// Read the request line off a connection + write its routed response. Best-
/// effort: a malformed/oversized request gets a 404, never a panic.
async fn handle_conn(mut stream: tokio::net::TcpStream, state: Arc<Mutex<ServeState>>) {
    let mut buf = vec![0u8; 8192];
    let n = match stream.read(&mut buf).await {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    let path = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string();
    let (status, ctype, body) = {
        let guard = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        route_request(&guard, &path)
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        reason = reason(status),
        len = body.len(),
    );
    let _ = stream.write_all(header.as_bytes()).await;
    let _ = stream.write_all(&body).await;
    let _ = stream.flush().await;
}

// ───────────────────────────── the worker ─────────────────────────────

/// MEDIA-15 — the mesh media server + DLNA + aggregation worker.
pub struct MediaServerWorker {
    /// This node's id (the publish stamp).
    node_id: String,
    /// This node's hostname (the plane key + the media host clients dial).
    hostname: String,
    /// The replicated QNM-Shared plane root peers read manifests off.
    mount: PathBuf,
    /// The chosen shared folders (`None` ⇒ [`load_share_dirs`] at run).
    share_dirs: Option<Vec<PathBuf>>,
    /// The mesh HTTP media-server port.
    port: u16,
    /// Rescan + republish cadence.
    tick: Duration,
    /// Unconditional-republish heartbeat.
    heartbeat: Duration,
    /// Bus root override (tests). `None` ⇒ `mde_bus::default_data_dir`.
    bus_root_override: Option<PathBuf>,
    /// Whether to bind the live HTTP server + attempt SSDP (off in tests).
    live: bool,
    /// Live-leg status: (http, ssdp), updated by the server tasks.
    live_status: Arc<Mutex<(String, String)>>,
    /// Fingerprint of the last published fold (publish-on-change gate).
    last_fingerprint: Option<String>,
}

impl MediaServerWorker {
    /// Construct with production seams + the default cadences. `node_id` stamps
    /// the publish, `hostname` keys the plane, `mount` is the replicated root.
    #[must_use]
    pub fn new(node_id: String, hostname: String, mount: PathBuf) -> Self {
        Self {
            node_id,
            hostname,
            mount,
            share_dirs: None,
            port: MESH_MEDIA_PORT,
            tick: DEFAULT_TICK_INTERVAL,
            heartbeat: PUBLISH_HEARTBEAT,
            bus_root_override: None,
            live: true,
            live_status: Arc::new(Mutex::new(("idle".to_string(), "idle".to_string()))),
            last_fingerprint: None,
        }
    }

    /// Override the chosen shared folders (tests / a spawn-site override).
    #[must_use]
    pub fn with_share_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.share_dirs = Some(dirs);
        self
    }

    /// Override the Bus spool root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    /// Override the tick cadence (tests avoid multi-second waits).
    #[must_use]
    pub const fn with_tick(mut self, d: Duration) -> Self {
        self.tick = d;
        self
    }

    /// Disable the live HTTP/SSDP legs (tests exercise manifest + aggregation
    /// without binding a socket).
    #[must_use]
    pub const fn without_live_server(mut self) -> Self {
        self.live = false;
        self
    }

    /// Resolve the chosen shared folders for this run.
    fn share_dirs(&self) -> Vec<PathBuf> {
        self.share_dirs.clone().unwrap_or_else(load_share_dirs)
    }

    /// Rescan this node's shared folders into a manifest + the serve map.
    fn rescan(&self) -> (ShareManifest, BTreeMap<String, PathBuf>) {
        build_manifest(&self.node_id, &self.hostname, self.port, &self.share_dirs())
    }

    /// Read every peer's manifest off the plane + fold them into the mesh
    /// library. This node's own freshly-written manifest is included (it lives
    /// on the same plane under its host dir).
    fn aggregate(&self) -> MeshLibrary {
        merge_libraries(&read_manifests(&self.mount))
    }

    /// Build this node's serving status from the current manifest + live-leg
    /// state.
    fn server_status(&self, manifest: &ShareManifest) -> ServerStatus {
        let (http, ssdp) = {
            let g = self
                .live_status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            g.clone()
        };
        ServerStatus {
            port: self.port,
            shared_folders: manifest.folders.clone(),
            shared_item_count: manifest.items.len(),
            http,
            ssdp,
        }
    }

    /// Publish the aggregated library when the fold changed (or `force`).
    /// Returns whether a record was written.
    fn publish(
        &mut self,
        persist: &Persist,
        manifest: &ShareManifest,
        library: MeshLibrary,
        force: bool,
    ) -> bool {
        let server = self.server_status(manifest);
        let fingerprint = serde_json::to_string(&(&server, &library)).unwrap_or_default();
        if !force && self.last_fingerprint.as_deref() == Some(fingerprint.as_str()) {
            return false;
        }
        let state = MeshLibraryState {
            node: self.node_id.clone(),
            server,
            library,
            published_at_ms: now_ms(),
        };
        let Ok(body) = serde_json::to_string(&state) else {
            return false;
        };
        if let Err(e) = persist.write(MESH_LIBRARY_TOPIC, Priority::Default, None, Some(&body)) {
            tracing::warn!(target: "mackesd::media_server", error = %e, "library publish failed");
            return false;
        }
        self.last_fingerprint = Some(fingerprint);
        true
    }

    /// One rescan→write-manifest→aggregate→publish cycle. Split out so the
    /// tick body is unit-tested without the run loop.
    fn tick_once(
        &mut self,
        persist: &Persist,
        serve: Option<&Arc<Mutex<ServeState>>>,
        force: bool,
    ) -> bool {
        let (manifest, abs) = self.rescan();
        write_manifest(&self.mount, &self.hostname, &manifest);
        if let Some(serve) = serve {
            let mut g = serve
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            g.manifest = manifest.clone();
            g.abs = abs;
        }
        // DLNA discovery announce (honestly gated live leg).
        if self.live {
            let uuid = stable_uuid(&self.hostname);
            match ssdp_announce(&uuid, &self.hostname, self.port) {
                Ok(()) => self.set_ssdp("ok (announced)".to_string()),
                Err(e) => self.set_ssdp(format!("gated: {e}")),
            }
        }
        let library = self.aggregate();
        self.publish(persist, &manifest, library, force)
    }

    fn set_http(&self, s: String) {
        self.live_status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0 = s;
    }

    fn set_ssdp(&self, s: String) {
        self.live_status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .1 = s;
    }

    /// Bind the mesh HTTP media server + spawn its accept loop. Best-effort: a
    /// bind failure sets a `gated:` http status (the worker keeps aggregating
    /// the mesh library); success means the descriptor probe finds
    /// `MESH_MEDIA_PORT` and peers' MEDIA-14 discovers this node.
    async fn start_http_server(&self, serve: Arc<Mutex<ServeState>>, mut shutdown: ShutdownToken) {
        let bind = format!("0.0.0.0:{}", self.port);
        let listener = match TcpListener::bind(&bind).await {
            Ok(l) => l,
            Err(e) => {
                self.set_http(format!("gated: bind {bind} failed ({e})"));
                return;
            }
        };
        self.set_http(format!("ok (serving {bind})"));
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = shutdown.wait() => break,
                    accept = listener.accept() => {
                        if let Ok((stream, _)) = accept {
                            let st = serve.clone();
                            tokio::spawn(handle_conn(stream, st));
                        }
                    }
                }
            }
        });
    }
}

#[async_trait::async_trait]
impl Worker for MediaServerWorker {
    fn name(&self) -> &'static str {
        "media_server"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self
            .bus_root_override
            .clone()
            .or_else(mde_bus::default_data_dir)
        else {
            tracing::debug!(target: "mackesd::media_server", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::media_server", error = %e, "persist open failed; worker idle");
                return Ok(());
            }
        };

        // The served state (manifest + serve map) the HTTP server reads.
        let serve = Arc::new(Mutex::new(ServeState {
            uuid: stable_uuid(&self.hostname),
            friendly_name: format!("{} (mde-media)", self.hostname),
            host: self.hostname.clone(),
            port: self.port,
            ..ServeState::default()
        }));

        if self.live {
            self.start_http_server(serve.clone(), shutdown.clone())
                .await;
        }

        // Immediate first cycle so a subscriber doesn't wait a tick.
        self.tick_once(&persist, Some(&serve), true);
        let mut last_pub = Instant::now();

        let mut tick = tokio::time::interval(self.tick);
        tick.tick().await; // burn the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let due = last_pub.elapsed() >= self.heartbeat;
                    if self.tick_once(&persist, Some(&serve), due) {
                        last_pub = Instant::now();
                    }
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

/// Wall-clock epoch millis for a published record.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── kind + mime classification ──

    #[test]
    fn ext_maps_to_media_kind_or_skips() {
        assert_eq!(media_kind_from_ext("mp4"), Some(MediaItemKind::Video));
        assert_eq!(media_kind_from_ext(".MKV"), Some(MediaItemKind::Video));
        assert_eq!(media_kind_from_ext("flac"), Some(MediaItemKind::Audio));
        assert_eq!(media_kind_from_ext("JPG"), Some(MediaItemKind::Image));
        // non-media is honestly skipped, never shared.
        assert_eq!(media_kind_from_ext("txt"), None);
        assert_eq!(media_kind_from_ext("iso"), None);
        assert_eq!(media_kind_from_ext(""), None);
    }

    #[test]
    fn mime_is_specific_then_kind_fallback() {
        assert_eq!(mime_for(MediaItemKind::Video, "mp4"), "video/mp4");
        assert_eq!(mime_for(MediaItemKind::Audio, "flac"), "audio/flac");
        assert_eq!(mime_for(MediaItemKind::Image, "png"), "image/png");
        // unknown-but-classified ext → kind fallback, never empty.
        assert_eq!(mime_for(MediaItemKind::Video, "xyz"), "video/octet-stream");
    }

    #[test]
    fn item_id_is_stable_and_collides_on_same_title_size() {
        // The SAME file (title+size) on two nodes yields the SAME id → the
        // aggregation dedups it. Different size → different id.
        assert_eq!(
            item_id("Big Buck Bunny", 1024),
            item_id("Big Buck Bunny", 1024)
        );
        assert_ne!(
            item_id("Big Buck Bunny", 1024),
            item_id("Big Buck Bunny", 2048)
        );
        assert_ne!(item_id("Sintel", 1024), item_id("Big Buck Bunny", 1024));
        assert_eq!(item_id("x", 1).len(), 16);
    }

    // ── the chosen shared folders ──

    #[test]
    fn resolve_share_dirs_env_wins() {
        let dirs = resolve_share_dirs(Some("/a:/b: :/c"), None, Path::new("/home/u"));
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c")
            ]
        );
    }

    #[test]
    fn resolve_share_dirs_config_then_default() {
        let cfg = r#"{"folders":["/media/movies","/media/music"]}"#;
        let dirs = resolve_share_dirs(None, Some(cfg), Path::new("/home/u"));
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/media/movies"),
                PathBuf::from("/media/music")
            ]
        );
        // No env + no/empty config → the standard media dirs under HOME.
        let def = resolve_share_dirs(None, None, Path::new("/home/u"));
        assert_eq!(
            def,
            vec![
                PathBuf::from("/home/u/Videos"),
                PathBuf::from("/home/u/Music"),
                PathBuf::from("/home/u/Pictures"),
            ]
        );
        // An empty env string falls through to the config/default, not [].
        let fell = resolve_share_dirs(Some("  "), None, Path::new("/home/u"));
        assert_eq!(fell.len(), 3);
    }

    // ── the manifest scan ──

    fn touch(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn build_manifest_scans_media_and_skips_non_media() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Videos");
        touch(&root.join("movie.mp4"), b"aaaa");
        touch(&root.join("song.flac"), b"bbbbbb");
        touch(&root.join("photo.jpg"), b"cc");
        touch(&root.join("notes.txt"), b"skip me"); // non-media → skipped
        touch(&root.join("sub/clip.mkv"), b"dddd"); // recursion
        let (m, abs) = build_manifest("peer:elm", "elm", MESH_MEDIA_PORT, &[root.clone()]);
        assert_eq!(m.node, "peer:elm");
        assert_eq!(m.host, "elm");
        assert_eq!(m.port, MESH_MEDIA_PORT);
        assert_eq!(m.folders, vec![root.to_string_lossy().into_owned()]);
        // 4 media files (txt skipped), each with an id in the serve map.
        assert_eq!(m.items.len(), 4);
        assert_eq!(abs.len(), 4);
        let titles: Vec<&str> = m.items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["clip", "movie", "photo", "song"]); // sorted
        let clip = m.items.iter().find(|i| i.title == "clip").unwrap();
        assert_eq!(clip.kind, MediaItemKind::Video);
        assert_eq!(clip.rel_path, "sub/clip.mkv");
        assert_eq!(clip.size_bytes, 4);
        assert!(abs.contains_key(&clip.id));
    }

    #[test]
    fn build_manifest_skips_missing_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("Music");
        touch(&real.join("a.mp3"), b"x");
        let missing = tmp.path().join("Nope");
        let (m, _) = build_manifest("n", "h", MESH_MEDIA_PORT, &[missing, real.clone()]);
        // Only the real folder is listed (a stale config folder never fabricates
        // a share).
        assert_eq!(m.folders, vec![real.to_string_lossy().into_owned()]);
        assert_eq!(m.items.len(), 1);
    }

    // ── the replicated plane I/O ──

    #[test]
    fn write_then_read_manifest_round_trips_across_hosts() {
        let tmp = tempfile::tempdir().unwrap();
        let mount = tmp.path();
        let (m_oak, _) = build_manifest("peer:oak", "oak", MESH_MEDIA_PORT, &[]);
        let (m_elm, _) = build_manifest("peer:elm", "elm", MESH_MEDIA_PORT, &[]);
        write_manifest(mount, "oak", &m_oak);
        write_manifest(mount, "elm", &m_elm);
        // The atomic write left no tmp file.
        assert!(!mount.join("oak").join(".media-library.json.tmp").exists());
        let back = read_manifests(mount);
        let hosts: Vec<&str> = back.iter().map(|m| m.host.as_str()).collect();
        assert_eq!(hosts, vec!["elm", "oak"]); // sorted by host
    }

    #[test]
    fn write_manifest_skips_empty_host() {
        let tmp = tempfile::tempdir().unwrap();
        let (m, _) = build_manifest("n", "", MESH_MEDIA_PORT, &[]);
        write_manifest(tmp.path(), "", &m);
        assert!(read_manifests(tmp.path()).is_empty());
    }

    // ── the aggregation merge fold (the acceptance-pinned deliverable) ──

    fn manifest_with(host: &str, items: &[(&str, u64, MediaItemKind)]) -> ShareManifest {
        let items = items
            .iter()
            .map(|(title, size, kind)| MediaItem {
                id: item_id(title, *size),
                title: (*title).to_string(),
                rel_path: format!("{title}.mp4"),
                kind: *kind,
                size_bytes: *size,
                mtime_ms: 0,
            })
            .collect();
        ShareManifest {
            node: format!("peer:{host}"),
            host: host.to_string(),
            port: MESH_MEDIA_PORT,
            folders: vec![],
            items,
            published_at_ms: 0,
        }
    }

    #[test]
    fn merge_unions_disjoint_items() {
        let oak = manifest_with("oak", &[("A", 1, MediaItemKind::Video)]);
        let elm = manifest_with("elm", &[("B", 2, MediaItemKind::Audio)]);
        let lib = merge_libraries(&[oak, elm]);
        assert_eq!(lib.item_count, 2);
        assert_eq!(lib.node_count, 2);
        let titles: Vec<&str> = lib.entries.iter().map(|e| e.item.title.as_str()).collect();
        assert_eq!(titles, vec!["A", "B"]); // sorted by title
    }

    #[test]
    fn merge_dedups_same_item_and_attributes_both_nodes() {
        // The SAME file (title+size) shared from oak AND elm collapses to ONE
        // entry attributed to both nodes (the per-node attribution requirement).
        let oak = manifest_with("oak", &[("Shared", 100, MediaItemKind::Video)]);
        let elm = manifest_with("elm", &[("Shared", 100, MediaItemKind::Video)]);
        let ash = manifest_with("ash", &[("Shared", 100, MediaItemKind::Video)]);
        let lib = merge_libraries(&[elm, oak, ash]);
        assert_eq!(lib.item_count, 1, "same file dedups to one entry");
        assert_eq!(lib.node_count, 3);
        assert_eq!(
            lib.entries[0].nodes,
            vec!["ash".to_string(), "elm".to_string(), "oak".to_string()],
            "nodes deduped + sorted"
        );
    }

    #[test]
    fn merge_different_sizes_stay_distinct() {
        // Same title but different size = different content = two entries.
        let oak = manifest_with("oak", &[("Movie", 100, MediaItemKind::Video)]);
        let elm = manifest_with("elm", &[("Movie", 200, MediaItemKind::Video)]);
        let lib = merge_libraries(&[oak, elm]);
        assert_eq!(lib.item_count, 2);
    }

    #[test]
    fn merge_empty_is_empty() {
        let lib = merge_libraries(&[]);
        assert_eq!(lib.item_count, 0);
        assert_eq!(lib.node_count, 0);
        assert!(lib.entries.is_empty());
    }

    // ── DLNA/UPnP pure XML builders ──

    #[test]
    fn stable_uuid_is_deterministic_and_shaped() {
        let a = stable_uuid("eagle");
        assert_eq!(a, stable_uuid("eagle"));
        assert_ne!(a, stable_uuid("oak"));
        // 8-4-4-4-12 uuid shape.
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
    }

    #[test]
    fn device_description_declares_a_mediaserver() {
        let xml = upnp_device_description("uuid-1", "elm (mde-media)", "10.42.0.2", 9600);
        assert!(xml.contains("urn:schemas-upnp-org:device:MediaServer:1"));
        assert!(xml.contains("urn:schemas-upnp-org:service:ContentDirectory:1"));
        assert!(xml.contains("<UDN>uuid:uuid-1</UDN>"));
        assert!(xml.contains("<friendlyName>elm (mde-media)</friendlyName>"));
        assert!(xml.contains("<URLBase>http://10.42.0.2:9600</URLBase>"));
    }

    #[test]
    fn didl_lite_carries_an_item_per_file_with_res_url() {
        let m = ShareManifest {
            items: vec![MediaItem {
                id: "abc123".into(),
                title: "A & B <Movie>".into(),
                rel_path: "A.mp4".into(),
                kind: MediaItemKind::Video,
                size_bytes: 42,
                mtime_ms: 0,
            }],
            ..ShareManifest::default()
        };
        let didl = didl_lite_from_manifest(&m, "http://10.42.0.2:9600");
        assert!(didl.contains("<item id=\"abc123\""));
        // Title is XML-escaped.
        assert!(didl.contains("A &amp; B &lt;Movie&gt;"));
        assert!(didl.contains("object.item.videoItem"));
        assert!(didl.contains("http://10.42.0.2:9600/media/abc123"));
        assert!(didl.contains("http-get:*:video/mp4:*"));
    }

    #[test]
    fn ssdp_notify_has_the_upnp_headers() {
        let n = ssdp_alive_notify("uuid-9", "10.42.0.2", 9600);
        assert!(n.starts_with("NOTIFY * HTTP/1.1\r\n"));
        assert!(n.contains("NTS: ssdp:alive\r\n"));
        assert!(n.contains("LOCATION: http://10.42.0.2:9600/description.xml\r\n"));
        assert!(n.contains("USN: uuid:uuid-9::urn:schemas-upnp-org:device:MediaServer:1\r\n"));
    }

    // ── the HTTP media-server routing ──

    fn serve_state_with(root: &Path) -> ServeState {
        let (manifest, abs) =
            build_manifest("peer:elm", "elm", MESH_MEDIA_PORT, &[root.to_path_buf()]);
        ServeState {
            manifest,
            abs,
            uuid: "uuid-x".into(),
            friendly_name: "elm (mde-media)".into(),
            host: "10.42.0.2".into(),
            port: MESH_MEDIA_PORT,
        }
    }

    #[test]
    fn route_serves_description_manifest_didl_and_media_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Videos");
        touch(&root.join("movie.mp4"), b"MOVIEBYTES");
        let state = serve_state_with(&root);
        let id = state.manifest.items[0].id.clone();

        // device description
        let (s, ct, body) = route_request(&state, "/description.xml");
        assert_eq!(s, 200);
        assert!(ct.contains("text/xml"));
        assert!(String::from_utf8_lossy(&body).contains("MediaServer:1"));

        // manifest.json round-trips to the manifest
        let (s, ct, body) = route_request(&state, "/manifest.json");
        assert_eq!(s, 200);
        assert_eq!(ct, "application/json");
        let back: ShareManifest = serde_json::from_slice(&body).unwrap();
        assert_eq!(back, state.manifest);

        // didl browse
        let (s, _, body) = route_request(&state, "/ContentDirectory/browse");
        assert_eq!(s, 200);
        assert!(String::from_utf8_lossy(&body).contains(&format!("/media/{id}")));

        // media bytes (with query string tolerated)
        let (s, ct, body) = route_request(&state, &format!("/media/{id}?x=1"));
        assert_eq!(s, 200);
        assert_eq!(ct, "video/mp4");
        assert_eq!(body, b"MOVIEBYTES");

        // unknown → 404
        let (s, _, _) = route_request(&state, "/nope");
        assert_eq!(s, 404);
        let (s, _, _) = route_request(&state, "/media/deadbeef");
        assert_eq!(s, 404);
    }

    // ── the live HTTP server (real bind, end-to-end) ──

    #[tokio::test]
    async fn live_http_server_serves_manifest_over_tcp() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Videos");
        touch(&root.join("movie.mp4"), b"HELLO");
        let (manifest, abs) = build_manifest("peer:elm", "elm", 0, &[root.clone()]);
        let serve = Arc::new(Mutex::new(ServeState {
            manifest: manifest.clone(),
            abs,
            uuid: "u".into(),
            friendly_name: "elm".into(),
            host: "127.0.0.1".into(),
            port: 0,
        }));
        // Bind an ephemeral port + run the accept loop.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (_tx, rx) = tokio::sync::watch::channel(false);
        let mut shutdown = ShutdownToken::from_receiver(rx);
        let serve_c = serve.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = shutdown.wait() => break,
                    accept = listener.accept() => {
                        if let Ok((stream, _)) = accept {
                            tokio::spawn(handle_conn(stream, serve_c.clone()));
                        }
                    }
                }
            }
        });

        // GET /manifest.json over a real TCP connection.
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /manifest.json HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).await.unwrap();
        let text = String::from_utf8_lossy(&resp);
        assert!(text.starts_with("HTTP/1.1 200 OK"));
        let body = text.split("\r\n\r\n").nth(1).unwrap();
        let back: ShareManifest = serde_json::from_str(body).unwrap();
        assert_eq!(back.items.len(), 1);
        assert_eq!(back.items[0].title, "movie");
    }

    // ── worker orchestration (no live server; real plane + bus) ──

    fn latest_state(persist: &Persist) -> MeshLibraryState {
        let msgs = persist.list_since(MESH_LIBRARY_TOPIC, None).unwrap();
        let body = msgs.last().unwrap().body.clone().unwrap();
        serde_json::from_str(&body).unwrap()
    }

    #[test]
    fn worker_writes_manifest_aggregates_and_publishes() {
        let bus = tempfile::tempdir().unwrap();
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let mount = tempfile::tempdir().unwrap();
        // A peer oak already published a manifest onto the plane.
        write_manifest(
            mount.path(),
            "oak",
            &manifest_with("oak", &[("PeerFilm", 500, MediaItemKind::Video)]),
        );
        // This node (elm) shares one local video.
        let share = tempfile::tempdir().unwrap();
        let root = share.path().join("Videos");
        touch(&root.join("Local.mp4"), b"local");

        let mut w =
            MediaServerWorker::new("peer:elm".into(), "elm".into(), mount.path().to_path_buf())
                .with_share_dirs(vec![root])
                .without_live_server();

        // First cycle writes elm's manifest + aggregates oak + elm + publishes.
        assert!(w.tick_once(&persist, None, true));
        // An unchanged fold doesn't republish; a forced (heartbeat) one does.
        assert!(!w.tick_once(&persist, None, false));
        assert!(w.tick_once(&persist, None, true));

        let state = latest_state(&persist);
        assert_eq!(state.node, "peer:elm");
        assert_eq!(state.server.shared_item_count, 1);
        assert_eq!(state.server.http, "idle"); // live server disabled in the test
                                               // The aggregated library carries BOTH oak's peer film + elm's local file.
        assert_eq!(state.library.node_count, 2);
        let titles: Vec<&str> = state
            .library
            .entries
            .iter()
            .map(|e| e.item.title.as_str())
            .collect();
        assert_eq!(titles, vec!["Local", "PeerFilm"]);
        // elm's own manifest landed on the plane for peers to aggregate.
        let planed = read_manifests(mount.path());
        assert!(planed.iter().any(|m| m.host == "elm"));
    }

    #[test]
    fn worker_name_is_media_server() {
        let w = MediaServerWorker::new("n".into(), "h".into(), PathBuf::from("/tmp/x"));
        assert_eq!(w.name(), "media_server");
        // The service MEDIA-14 discovery keys a mesh player off (`mde-media`) is
        // the same token descriptors::MEDIA_PORTS advertises when MESH_MEDIA_PORT
        // is bound — the producer ⇄ consumer contract, pinned.
        assert_eq!(
            super::super::media_sources::SERVICE_MESH_PLAYER,
            "mde-media"
        );
    }
}
