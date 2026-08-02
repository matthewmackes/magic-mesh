//! `BusBackend` — mesh-**Bus** client for mackesd's Fleet.Files
//! surface (E0.3.2). Migrated off the `dev.mackes.MDE.Fleet.Files`
//! D-Bus proxy onto the Bus action/reply pattern: the mesh peer
//! roster reads over `action/fleet-files/{self-node,peers,list-peer}`
//! (the `list-peer` verb carries its peer name in the request body),
//! mirroring the [`crate::mesh_backend::MeshBackend`] Nebula client
//! that E0.3.1.a already moved to the Bus.
//!
//! Gated behind the `dbus` cargo feature (the mackesd-IPC dep group —
//! tokio + mde-bus) so the headless DemoBackend smoke build keeps a
//! minimal dep graph. (The feature name is a pre-E0.3 artifact; the
//! transport is the Bus now, not D-Bus.)
//!
//! Like `MeshBackend`, a tokio runtime + the resolved Bus data dir are
//! held; each call opens a fresh `Persist` inside `rt.block_on` (it
//! isn't `Send`) and blocks the caller until mackesd's responder
//! replies, bounded by a per-call timeout so the GUI thread never
//! freezes. The wire types + parsers + model bridges + selector
//! grammar are transport-agnostic and unchanged from the Phase-2.3
//! scaffold.

#![cfg(feature = "dbus")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use mackes_mesh_types::cloud::{
    cloud_request_digest, decode_cloud_arm_credential, CloudArmSigner, CloudArmedToken,
    CLOUD_ACTION_SCHEMA_VERSION, CLOUD_ARM_CREDENTIAL,
};
use serde::Deserialize;
use tokio::runtime::Runtime;
use mackes_mesh_types::service_record::{ServiceHealth, ServicesState};

use crate::backend::{BackendError, ConflictPolicy, Destination, OpId, SendMode};
use crate::model::{FileRow, Mime, Peer, PeerKind, PeerStatus, SelfNode};

/// Action-topic prefix for mackesd's Fleet.Files Bus surface — must
/// equal `mackesd_core::ipc::files::FLEET_FILES_PREFIX`.
pub const FLEET_FILES_PREFIX: &str = "fleet-files";
/// AUD-1 — the file-operations surface (Send-To / rollback / audit-log).
pub const FILE_OPS_PREFIX: &str = "file-ops";
/// Capability verb consumed by mackesd's `action/file-ops/send-to` gate.
const FILE_OPS_SEND_TO_AUTH_VERB: &str = "file-ops-send-to";
/// Keep a file-operation capability useful for one Bus drain only.
const ACTION_TOKEN_TTL_MS: i64 = 30_000;
/// AUD-1/AUD-7 — the replicated inbox surface (received files).
pub const INBOX_PREFIX: &str = "files-inbox";

/// Resolve the host identity used by mackesd's file responder for the
/// capability's node context. `/etc/hostname` is authoritative for that
/// responder; the environment fallback keeps desktop/test launches useful.
fn local_node() -> String {
    for path in ["/etc/hostname", "/proc/sys/kernel/hostname"] {
        if let Ok(hostname) = std::fs::read_to_string(path) {
            let hostname = hostname.trim();
            if !hostname.is_empty() {
                return hostname.to_string();
            }
        }
    }
    std::env::var("HOSTNAME")
        .ok()
        .map(|hostname| hostname.trim().to_string())
        .filter(|hostname| !hostname.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// The root/systemd credential loader shared by the privileged Bus
/// producers. A missing credential, an unprivileged process, or an invalid
/// credential is an authorization failure, never a reason to publish an
/// unsigned mutation.
fn action_signer() -> Result<CloudArmSigner, String> {
    #[cfg(test)]
    {
        return CloudArmSigner::new(b"0123456789abcdef0123456789abcdef".to_vec())
            .map_err(str::to_string);
    }
    #[cfg(not(test))]
    {
        if !running_as_root() {
            return Err("file-operation authorization requires the root service process".into());
        }
        let directory = std::env::var_os("CREDENTIALS_DIRECTORY")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| "systemd action credential is unavailable".to_string())?;
        load_action_signer(&directory, true)
    }
}

/// Load one credential from an explicit directory. The boolean is injectable
/// only so hostile tests can prove the root gate without changing process
/// credentials or global environment state.
fn load_action_signer(directory: &Path, running_as_root: bool) -> Result<CloudArmSigner, String> {
    if !running_as_root {
        return Err("file-operation authorization requires the root service process".into());
    }
    let path = directory.join(CLOUD_ARM_CREDENTIAL);
    let raw = std::fs::read(&path)
        .map_err(|error| format!("read systemd action credential {}: {error}", path.display()))?;
    let key = decode_cloud_arm_credential(&raw).map_err(str::to_string)?;
    CloudArmSigner::new(key).map_err(str::to_string)
}

/// Linux's standard-library-only euid probe keeps this service crate's
/// dependency graph unchanged while retaining the root-only signer boundary.
#[cfg(not(test))]
fn running_as_root() -> bool {
    #[cfg(target_os = "linux")]
    {
        return std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                status.lines().find_map(|line| {
                    let mut fields = line.split_whitespace();
                    if fields.next() == Some("Uid:") {
                        fields.nth(1).and_then(|euid| euid.parse::<u32>().ok())
                    } else {
                        None
                    }
                })
            })
            == Some(0);
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Mint a cryptographically random, single-use nonce without adding a
/// second RNG dependency to this render-agnostic service crate.
fn fresh_nonce() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    let mut random = std::fs::File::open("/dev/urandom")
        .map_err(|error| format!("open system random source: {error}"))?;
    std::io::Read::read_exact(&mut random, &mut bytes)
        .map_err(|error| format!("read system random source: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Add schema v1 and an exact-body, short-lived capability to one
/// `action/file-ops/send-to` request. The four pre-existing business fields
/// remain unchanged; only the required auth envelope is additive.
fn authorize_send_to_body(unsigned_body: &str) -> Result<String, String> {
    let mut document: serde_json::Value = serde_json::from_str(unsigned_body)
        .map_err(|error| format!("invalid file-operation request body: {error}"))?;
    let object = document
        .as_object_mut()
        .ok_or_else(|| "file-operation request body must be a JSON object".to_string())?;
    object.remove("armed_token");
    object.insert(
        "schema_version".to_string(),
        serde_json::Value::from(CLOUD_ACTION_SCHEMA_VERSION),
    );
    let selector = object
        .get("selector")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let peer = selector
        .strip_prefix("peer:")
        .filter(|peer| {
            !peer.is_empty()
                && *peer != "."
                && *peer != ".."
                && !peer.contains('/')
                && !peer.contains('\\')
                && !peer.contains('\0')
        })
        .ok_or_else(|| {
            format!(
                "file-operation authorization requires a clean peer selector (got '{selector}')"
            )
        })?;
    let node = local_node();
    let target = format!("peer:{peer}");
    for (label, value) in [
        ("verb", FILE_OPS_SEND_TO_AUTH_VERB),
        ("node", node.as_str()),
        ("target", target.as_str()),
    ] {
        if value.contains('|') || value.len() > 255 || value.trim().is_empty() {
            return Err(format!(
                "file-operation authorization {label} is not capability-safe"
            ));
        }
    }
    let unsigned = document.to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .map_err(|_| "system clock is before the Unix epoch".to_string())?;
    let nonce = fresh_nonce()?;
    let signer = action_signer()?;
    let token = CloudArmedToken::mint(
        &signer,
        &nonce,
        now.saturating_add(ACTION_TOKEN_TTL_MS),
        FILE_OPS_SEND_TO_AUTH_VERB,
        &node,
        &target,
        &cloud_request_digest(&unsigned).map_err(str::to_string)?,
    )
    .encode();
    document
        .as_object_mut()
        .expect("validated JSON object")
        .insert("armed_token".to_string(), serde_json::Value::String(token));
    serde_json::to_string(&document)
        .map_err(|error| format!("encode authorized file-operation request: {error}"))
}

/// E10 — one row of the `action/connect/devices` KDE-Connect roster reply
/// (mirrors the shell's `connect::WireDevice`). Extra fields are ignored.
#[derive(Deserialize)]
struct CloudWireDevice {
    name: String,
    #[serde(default)]
    online: bool,
    #[serde(default)]
    battery: Option<u8>,
}

/// Decode the `action/connect/devices` roster JSON into Cloud-Files device
/// rows (online state in the size column, battery in the age column). Pure —
/// unit-tested. Malformed JSON yields no rows (the honest empty state).
fn cloud_rows_from_json(raw: &str) -> Vec<FileRow> {
    let wires: Vec<CloudWireDevice> = serde_json::from_str(raw).unwrap_or_default();
    wires
        .into_iter()
        .map(|w| {
            let status = if w.online { "online" } else { "offline" };
            let batt = w
                .battery
                .filter(|b| *b <= 100)
                .map(|b| format!("battery {b}%"))
                .unwrap_or_else(|| "—".to_string());
            FileRow::local(w.name, Mime::Folder, status, batt)
        })
        .collect()
}

/// Wire-format `SelfNode` as mackesd encodes it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WireSelfNode {
    pub host: String,
    pub role: String,
    pub region: String,
}

/// Wire-format `Peer` row.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WirePeer {
    pub name: String,
    pub addr: String,
    pub kind: String,
    pub status: String,
}

/// Wire-format `FileRow`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WireFileRow {
    pub name: String,
    pub size: u64,
    pub mime: String,
    pub peer: String,
    pub modified_ms: i64,
}

/// Wire-format audit row.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WireAudit {
    pub op_id: u64,
    pub kind: String,
    pub source: String,
    pub destination: String,
    pub mode: String,
    pub bytes: u64,
    pub at_ms: i64,
    pub ok: bool,
}

/// Cheap-to-construct Fleet.Files Bus client. A tokio runtime + the
/// resolved Bus data dir are held; each call opens a fresh `Persist`
/// inside `rt.block_on` and blocks until mackesd's responder replies.
pub struct BusBackend {
    rt: Runtime,
    /// Bus data dir. A fresh `Persist` is opened from it per call
    /// rather than held here, because `Persist` (rusqlite) is not
    /// `Send` and `BusBackend` lives inside the UI backend.
    bus_dir: PathBuf,
    /// Per-call timeout — keeps the GUI thread snappy when mackesd is
    /// busy.
    call_timeout: Duration,
}

impl BusBackend {
    /// Default connect-probe timeout.
    pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_millis(800);
    /// Default per-method call timeout.
    pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_millis(750);

    /// Connect with the default timeout.
    ///
    /// # Errors
    ///
    /// Returns `BackendError::Rejected` when the runtime can't build,
    /// no Bus data dir resolves, or the liveness probe times out.
    pub fn connect() -> Result<Self, BackendError> {
        Self::connect_with_timeout(Self::DEFAULT_CONNECT_TIMEOUT)
    }

    /// Resolve the Bus data dir + verify mackesd's Fleet.Files
    /// responder is live with a single `action/fleet-files/self-node`
    /// round-trip. Preserves the old fast-fail contract callers
    /// expect (`Err` => the backend falls back to local) but over the
    /// Bus rather than a D-Bus `NameHasOwner` check.
    ///
    /// # Errors
    ///
    /// Returns `BackendError::Rejected` when the runtime fails to
    /// build, no Bus data dir resolves, the persist can't open, or the
    /// probe times out (no responder).
    pub fn connect_with_timeout(timeout: Duration) -> Result<Self, BackendError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| BackendError::Rejected(format!("tokio runtime: {e}")))?;
        let bus_dir = mde_bus::client_data_dir()
            .ok_or_else(|| BackendError::Rejected("no Bus data dir".into()))?;
        rt.block_on(async {
            let persist = mde_bus::persist::Persist::open(bus_dir.clone())
                .map_err(|e| BackendError::Rejected(format!("bus persist: {e}")))?;
            mde_bus::rpc::request(
                &persist,
                "action/fleet-files/self-node",
                mde_bus::hooks::config::Priority::Default,
                None,
                None,
                timeout,
            )
            .await
            .map(|_| ())
            .map_err(|e| BackendError::Rejected(format!("fleet-files probe: {e}")))
        })?;
        Ok(Self {
            rt,
            bus_dir,
            call_timeout: Self::DEFAULT_CALL_TIMEOUT,
        })
    }

    /// Override the per-call timeout. Tests use this to stay fast.
    #[must_use]
    pub fn with_call_timeout(mut self, t: Duration) -> Self {
        self.call_timeout = t;
        self
    }

    /// Whether the mesh currently has a probe-confirmed music service.
    /// Published-only records are deliberately not enough to expose an
    /// uploader: Files must only offer a route the daemon can reach now.
    #[must_use]
    pub fn music_service_available(&self) -> bool {
        let Ok(persist) = mde_bus::persist::Persist::open(self.bus_dir.clone()) else {
            return false;
        };
        persist
            .list_topics()
            .ok()
            .into_iter()
            .flatten()
            .filter(|topic| topic.starts_with("state/services/"))
            .filter_map(|topic| persist.read_latest(&topic).ok().flatten())
            .filter_map(|message| message.body)
            .filter_map(|body| serde_json::from_str::<ServicesState>(&body).ok())
            .any(|state| {
                state.records.iter().any(|record| {
                    matches!(record.health, ServiceHealth::Up)
                        && matches!(record.kind.to_ascii_lowercase().as_str(), "airsonic" | "navidrome")
                })
            })
    }

    /// Fetch the JSON-encoded `SelfNode` and decode into the UI model.
    ///
    /// # Errors
    ///
    /// `BackendError::Rejected` when the Bus call fails or the body
    /// fails to decode.
    pub fn self_node(&self) -> Result<SelfNode, BackendError> {
        let raw = self.bus_request("self-node", None)?;
        let w = parse_self_node(&raw)
            .ok_or_else(|| BackendError::Rejected(format!("self_node decode failed: {raw}")))?;
        Ok(SelfNode {
            id: format!("self:{}", w.host),
            host: w.host,
            label: "this node".into(),
            addr: w.region,
            files: 0,
            shared: 0,
        })
    }

    /// Fetch the JSON-encoded peers array and decode into UI [`Peer`]s.
    ///
    /// # Errors
    ///
    /// `BackendError::Rejected` when the Bus call fails or the body
    /// fails to decode.
    pub fn peers(&self) -> Result<Vec<Peer>, BackendError> {
        let raw = self.bus_request("peers", None)?;
        let wires = parse_peers(&raw)
            .ok_or_else(|| BackendError::Rejected(format!("peers decode failed: {raw}")))?;
        Ok(wires.into_iter().map(WirePeer::into_model).collect())
    }

    /// Fetch the JSON-encoded list of files visible under a peer (the
    /// peer name travels in the request body).
    ///
    /// # Errors
    ///
    /// `BackendError::Rejected` when the Bus call fails or the body
    /// fails to decode.
    pub fn list_peer(&self, peer: &str) -> Result<Vec<FileRow>, BackendError> {
        let raw = self.bus_request("list-peer", Some(peer))?;
        let wires = parse_files(&raw)
            .ok_or_else(|| BackendError::Rejected(format!("list_peer decode failed: {raw}")))?;
        Ok(wires.into_iter().map(WireFileRow::into_model).collect())
    }

    /// E10 — the paired KDE-Connect device roster over the Bus
    /// (`action/connect/devices`, the mackesd KDC host worker), surfaced as
    /// Cloud-Files device rows. Empty (never errors) when the daemon / Bus /
    /// reply is unavailable, so the view renders an honest "no devices" state.
    pub fn cloud_devices(&self) -> Vec<FileRow> {
        let raw: Option<String> = self.rt.block_on(async {
            let persist = mde_bus::persist::Persist::open(self.bus_dir.clone()).ok()?;
            mde_bus::rpc::request(
                &persist,
                "action/connect/devices",
                mde_bus::hooks::config::Priority::Default,
                None,
                None,
                self.call_timeout,
            )
            .await
            .ok()?
            .body
        });
        raw.as_deref().map(cloud_rows_from_json).unwrap_or_default()
    }

    /// AUD-7 — this node's replicated mesh inbox (files other peers sent us),
    /// over `action/files-inbox/list`. Empty (never errors) when the daemon /
    /// Bus / reply is unavailable, so the Inbox view degrades honestly.
    pub fn inbox(&self) -> Vec<FileRow> {
        self.bus_request_at(INBOX_PREFIX, "list", None)
            .ok()
            .and_then(|raw| parse_files(&raw))
            .map(|wires| wires.into_iter().map(WireFileRow::into_model).collect())
            .unwrap_or_default()
    }

    /// AUD-1 — fire a Send-To over `action/file-ops/send-to`: the sender's
    /// mackesd copies each source into the target peer's replicated inbox.
    /// Returns the op id mackesd assigned.
    ///
    /// # Errors
    ///
    /// `BackendError::Rejected` when the Bus call fails or mackesd rejects
    /// the send (e.g. a non-`peer:` destination, unreadable source).
    pub fn send_to(
        &self,
        sources: &[PathBuf],
        destination: &Destination,
        mode: SendMode,
        conflict: ConflictPolicy,
    ) -> Result<OpId, BackendError> {
        let unsigned_body = serde_json::json!({
            "sources": sources.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
            "selector": destination_to_selector(destination),
            "mode": send_mode_to_str(mode),
            "conflict": conflict_policy_to_str(conflict),
        })
        .to_string();
        let body = authorize_send_to_body(&unsigned_body)
            .map_err(|error| BackendError::Rejected(format!("send-to authorization: {error}")))?;
        let raw = self.bus_request_at(FILE_OPS_PREFIX, "send-to", Some(&body))?;
        let v: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| BackendError::Rejected(format!("send-to decode: {e}")))?;
        if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
            let op = v
                .get("op_id")
                .and_then(serde_json::Value::as_i64)
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or_default();
            Ok(op)
        } else {
            Err(BackendError::Rejected(
                v.get("error")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("send-to rejected")
                    .to_string(),
            ))
        }
    }

    /// Publish one `action/fleet-files/<verb>` request on the Bus
    /// (carrying `body` for arg-bearing verbs) + block for the reply
    /// body. A fresh `Persist` is opened per call (it isn't `Send`);
    /// `call_timeout` bounds the wait. `Err` on timeout / no-responder
    /// / empty reply — callers map that to their fallback path, exactly
    /// as the old D-Bus errors did. An `{"error":…}` reply envelope is
    /// not a JSON array/object the parsers accept, so it surfaces as a
    /// decode `Rejected` upstream.
    fn bus_request(&self, verb: &str, body: Option<&str>) -> Result<String, BackendError> {
        self.bus_request_at(FLEET_FILES_PREFIX, verb, body)
    }

    /// AUD-1 — issue `action/<prefix>/<verb>` (carrying `body`) + block for
    /// the reply, for surfaces other than Fleet.Files (file-ops, inbox).
    /// Same contract as [`Self::bus_request`].
    fn bus_request_at(
        &self,
        prefix: &str,
        verb: &str,
        body: Option<&str>,
    ) -> Result<String, BackendError> {
        let topic = format!("action/{prefix}/{verb}");
        self.rt.block_on(async {
            let persist = mde_bus::persist::Persist::open(self.bus_dir.clone())
                .map_err(|e| BackendError::Rejected(format!("bus persist: {e}")))?;
            match mde_bus::rpc::request(
                &persist,
                &topic,
                mde_bus::hooks::config::Priority::Default,
                None,
                body,
                self.call_timeout,
            )
            .await
            {
                Ok(reply) => reply
                    .body
                    .ok_or_else(|| BackendError::Rejected(format!("{topic}: empty reply"))),
                Err(e) => Err(BackendError::Rejected(format!("{topic}: {e}"))),
            }
        })
    }
}

// ---- wire → UI model bridges -------------------------------------

impl WirePeer {
    /// Translate the JSON-wire peer into the UI's [`Peer`] type.
    /// Unknown `kind`/`status` strings fall back to sensible defaults
    /// so an unrecognised peer still renders rather than disappearing.
    #[must_use]
    pub fn into_model(self) -> Peer {
        let kind = match self.kind.as_str() {
            "server" | "nas" => PeerKind::Server,
            "phone" | "mobile" => PeerKind::Phone,
            "ci" | "runner" => PeerKind::Ci,
            _ => PeerKind::Desktop,
        };
        let status = match self.status.as_str() {
            "online" | "healthy" => PeerStatus::Online,
            "idle" | "degraded" => PeerStatus::Idle,
            _ => PeerStatus::Offline,
        };
        Peer {
            id: self.name.clone(),
            host: format!("{}.mesh", self.name),
            label: self.name.clone(),
            kind,
            addr: self.addr,
            status,
            latency: None,
            files: 0,
            shared: 0,
            last: String::new(),
        }
    }
}

impl WireFileRow {
    /// Translate the JSON-wire file row into the UI's [`FileRow`]
    /// type. Sizes get formatted via the shared `fmt_bytes` helper;
    /// modified-ms turns into a relative-age string ("4 min", "1 h").
    #[must_use]
    pub fn into_model(self) -> FileRow {
        let mime = match self.mime.as_str() {
            "folder" | "dir" => Mime::Folder,
            "image" | "img" => Mime::Image,
            "pdf" => Mime::Pdf,
            "archive" | "zip" | "tar" => Mime::Archive,
            "disk" | "iso" | "qcow2" => Mime::Disk,
            _ => Mime::Doc,
        };
        let row = FileRow::local(
            self.name,
            mime,
            fmt_bytes_u64(self.size),
            fmt_age_ms(self.modified_ms),
        );
        if self.peer.is_empty() {
            row
        } else {
            row.with_from(self.peer)
        }
    }
}

fn fmt_bytes_u64(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if n >= GB {
        format!("{:.1} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{} KB", n / KB)
    } else {
        format!("{n} B")
    }
}

fn fmt_age_ms(modified_ms: i64) -> String {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let delta = (now_ms - modified_ms).max(0);
    let secs = delta / 1000;
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min", secs / 60)
    } else if secs < 86_400 {
        format!("{} h", secs / 3600)
    } else if secs < 30 * 86_400 {
        format!("{} d", secs / 86_400)
    } else {
        "—".into()
    }
}

// ---- pure parsers (testable, no I/O) -----------------------------

/// Parse the JSON-encoded SelfNode mackesd returns.
#[must_use]
pub fn parse_self_node(raw: &str) -> Option<WireSelfNode> {
    serde_json::from_str(raw).ok()
}

/// Parse a JSON array of peers.
#[must_use]
pub fn parse_peers(raw: &str) -> Option<Vec<WirePeer>> {
    serde_json::from_str(raw).ok()
}

/// Parse a JSON array of file rows.
#[must_use]
pub fn parse_files(raw: &str) -> Option<Vec<WireFileRow>> {
    serde_json::from_str(raw).ok()
}

/// Parse the JSON-encoded audit log.
#[must_use]
pub fn parse_audit(raw: &str) -> Option<Vec<WireAudit>> {
    serde_json::from_str(raw).ok()
}

/// Encode a `Destination` into the mackesd selector grammar.
#[must_use]
pub fn destination_to_selector(d: &Destination) -> String {
    match d {
        Destination::Peer(n) => format!("peer:{n}"),
        Destination::Group(g) => format!("group:{g}"),
        Destination::Role(r) => format!("role:{r}"),
        Destination::Site(s) => format!("site:{s}"),
    }
}

/// Inverse: parse the mackesd selector grammar.
#[must_use]
pub fn parse_destination(raw: &str) -> Destination {
    if let Some(rest) = raw.strip_prefix("peer:") {
        Destination::Peer(rest.to_string())
    } else if let Some(rest) = raw.strip_prefix("group:") {
        Destination::Group(rest.to_string())
    } else if let Some(rest) = raw.strip_prefix("role:") {
        Destination::Role(rest.to_string())
    } else if let Some(rest) = raw.strip_prefix("site:") {
        Destination::Site(rest.to_string())
    } else {
        Destination::Peer(raw.to_string())
    }
}

#[must_use]
pub fn send_mode_to_str(m: SendMode) -> &'static str {
    match m {
        SendMode::Copy => "copy",
        SendMode::Move => "move",
        SendMode::Sync => "sync",
        SendMode::Deploy => "deploy",
        SendMode::Stage => "stage",
    }
}

#[must_use]
pub fn parse_send_mode(s: &str) -> SendMode {
    match s {
        "move" => SendMode::Move,
        "sync" => SendMode::Sync,
        "deploy" => SendMode::Deploy,
        "stage" => SendMode::Stage,
        _ => SendMode::Copy,
    }
}

#[must_use]
pub fn conflict_policy_to_str(c: ConflictPolicy) -> &'static str {
    match c {
        ConflictPolicy::Ask => "ask",
        ConflictPolicy::Skip => "skip",
        ConflictPolicy::Overwrite => "overwrite",
        ConflictPolicy::Rename => "rename",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_rows_decode_roster_and_map_status_battery() {
        let raw = r#"[
            {"id":"a","name":"Pixel 8","online":true,"battery":85},
            {"id":"b","name":"Old Tablet","online":false,"battery":null}
        ]"#;
        let rows = cloud_rows_from_json(raw);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Pixel 8");
        assert_eq!(rows[0].size, "online");
        assert_eq!(rows[0].age, "battery 85%");
        assert!(rows[0].is_dir()); // a device browses like a folder
        assert_eq!(rows[1].size, "offline");
        assert_eq!(rows[1].age, "—");
        // Malformed JSON → honest empty state, never panics.
        assert!(cloud_rows_from_json("not json").is_empty());
    }

    #[test]
    fn parse_self_node_round_trips_basic_shape() {
        let raw = r#"{"host":"anvil","role":"editor","region":"lab"}"#;
        let n = parse_self_node(raw).expect("decoded");
        assert_eq!(n.host, "anvil");
        assert_eq!(n.role, "editor");
        assert_eq!(n.region, "lab");
    }

    #[test]
    fn parse_self_node_returns_none_on_garbage() {
        assert!(parse_self_node("not json").is_none());
    }

    #[test]
    fn parse_self_node_rejects_error_envelope() {
        // An `{"error":…}` reply from the responder is not a SelfNode;
        // the parser returns None so the caller surfaces a decode err.
        assert!(parse_self_node(r#"{"error":"list_nodes: boom"}"#).is_none());
    }

    #[test]
    fn parse_peers_decodes_array() {
        let raw = r#"[
            {"name":"pine","addr":"10.0.0.1","kind":"laptop","status":"online"},
            {"name":"birch","addr":"10.0.0.2","kind":"server","status":"offline"}
        ]"#;
        let peers = parse_peers(raw).expect("decoded");
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].name, "pine");
        assert_eq!(peers[1].kind, "server");
        assert_eq!(peers[1].status, "offline");
    }

    #[test]
    fn parse_files_decodes_rows() {
        let raw = r#"[
            {"name":"notes.md","size":1234,"mime":"doc","peer":"pine","modified_ms":1715000000000}
        ]"#;
        let rows = parse_files(raw).expect("decoded");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "notes.md");
        assert_eq!(rows[0].size, 1234);
    }

    #[test]
    fn parse_audit_round_trips_basic_row() {
        let raw = r#"[
            {"op_id":42,"kind":"send_to","source":"/tmp/a","destination":"peer:pine","mode":"copy","bytes":4096,"at_ms":1715000000000,"ok":true}
        ]"#;
        let rows = parse_audit(raw).expect("decoded");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].op_id, 42);
        assert_eq!(rows[0].kind, "send_to");
        assert_eq!(rows[0].destination, "peer:pine");
    }

    #[test]
    fn destination_selector_round_trip() {
        for d in [
            Destination::Peer("pine".into()),
            Destination::Group("crew".into()),
            Destination::Role("editor".into()),
            Destination::Site("lab".into()),
        ] {
            let s = destination_to_selector(&d);
            assert_eq!(parse_destination(&s), d);
        }
    }

    #[test]
    fn parse_destination_falls_back_to_peer_on_unknown_prefix() {
        let d = parse_destination("nothing-prefixed");
        assert_eq!(d, Destination::Peer("nothing-prefixed".into()));
    }

    #[test]
    fn send_mode_round_trip() {
        for m in [
            SendMode::Copy,
            SendMode::Move,
            SendMode::Sync,
            SendMode::Deploy,
            SendMode::Stage,
        ] {
            assert_eq!(parse_send_mode(send_mode_to_str(m)), m);
        }
    }

    #[test]
    fn conflict_policy_to_str_covers_all_variants() {
        assert_eq!(conflict_policy_to_str(ConflictPolicy::Ask), "ask");
        assert_eq!(conflict_policy_to_str(ConflictPolicy::Skip), "skip");
        assert_eq!(
            conflict_policy_to_str(ConflictPolicy::Overwrite),
            "overwrite"
        );
        assert_eq!(conflict_policy_to_str(ConflictPolicy::Rename), "rename");
    }

    #[test]
    fn send_to_body_keeps_wire_fields_and_binds_schema_v1_to_exact_body() {
        let unsigned = serde_json::json!({
            "sources": ["/home/mm/share/report.pdf"],
            "selector": "peer:oak",
            "mode": "copy",
            "conflict": "rename",
        })
        .to_string();
        let body = authorize_send_to_body(&unsigned).expect("authorized body");
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(value["schema_version"], CLOUD_ACTION_SCHEMA_VERSION);
        assert_eq!(
            value["sources"],
            serde_json::json!(["/home/mm/share/report.pdf"])
        );
        assert_eq!(value["selector"], "peer:oak");
        assert_eq!(value["mode"], "copy");
        assert_eq!(value["conflict"], "rename");

        let token = CloudArmedToken::parse(value["armed_token"].as_str().expect("token"))
            .expect("well-formed capability");
        assert_eq!(token.verb, FILE_OPS_SEND_TO_AUTH_VERB);
        assert_eq!(token.node, local_node());
        assert_eq!(token.target, "peer:oak");
        let signer = action_signer().expect("test signer");
        assert!(signer.verify_payload(&token.signing_payload(), &token.signature));

        let mut unsigned_value = value.clone();
        unsigned_value
            .as_object_mut()
            .expect("object")
            .remove("armed_token");
        assert_eq!(
            token.request_sha256,
            cloud_request_digest(&unsigned_value.to_string()).expect("digest")
        );

        // Any body edit, even one that leaves the selector/target unchanged,
        // receives a different digest and cannot reuse this capability.
        unsigned_value["sources"][0] = serde_json::Value::String("/etc/shadow".into());
        assert_ne!(
            token.request_sha256,
            cloud_request_digest(&unsigned_value.to_string()).expect("tampered digest")
        );
    }

    #[test]
    fn send_to_body_rejects_hostile_peer_selector_before_publication() {
        for selector in ["peer:../oak", "peer:oak/../../etc", "peer:oak|forged"] {
            let unsigned = serde_json::json!({
                "sources": ["/home/mm/share/report.pdf"],
                "selector": selector,
                "mode": "copy",
                "conflict": "rename",
            })
            .to_string();
            assert!(
                authorize_send_to_body(&unsigned).is_err(),
                "hostile selector was accepted: {selector}"
            );
        }
    }

    #[test]
    fn action_signer_fails_closed_for_missing_credential_or_non_root() {
        assert!(load_action_signer(Path::new("/definitely/missing"), true).is_err());
        assert!(load_action_signer(Path::new("/"), false).is_err());
    }

    #[test]
    fn topic_prefix_matches_mackesd() {
        // Cross-check: must equal mackesd_core::ipc::files::FLEET_FILES_PREFIX.
        assert_eq!(FLEET_FILES_PREFIX, "fleet-files");
    }
}
