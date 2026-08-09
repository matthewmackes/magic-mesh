//! WL-ARCH-009 single-writer boundary for the persistent mackesd store.
//!
//! The control group owns one connection behind this local, typed, bounded
//! Unix-socket protocol. Canonical store-helper mutations use it in split
//! `serve` mode. Unconverted direct-SQL paths deliberately retain their prior
//! connection behavior until migrated; the authority lint inventories them and
//! prevents that residual set from growing. This module never accepts SQL text.

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::Result;

pub const SCHEMA_VERSION: u16 = 1;
/// Maximum writer request/response frame. CA disaster-recovery archives are
/// independently capped at one MiB before they reach this boundary, so leave
/// finite JSON framing headroom without admitting an unbounded request.
pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
const MAX_IDENTITY_BYTES: usize = 255;
const MAX_NAME_BYTES: usize = 256;
const MAX_REGION_BYTES: usize = 256;
const MAX_PUBLIC_KEY_BYTES: usize = 16 * 1024;
const MAX_CERT_PEM_BYTES: usize = 64 * 1024;
const MAX_CA_PAYLOAD_BYTES: usize = 1024 * 1024;
const MAX_EVENT_DETAIL_BYTES: usize = 256 * 1024;
const MAX_REVISION_SPEC_BYTES: usize = 1024 * 1024;
const MAX_SUMMARY_BYTES: usize = 4096;
const MAX_FLEET_PEERS: usize = 4096;
const MAX_RECONCILE_EVENTS: usize = 4096;
const MAX_CLOCK_SNAPSHOT_BYTES: usize = 256 * 1024;
const MAX_CLOCK_AUDIO_OUTBOX_ROWS: usize = 512;
const MAX_JSON_DEPTH: usize = 32;
const MAX_JSON_CONTAINER_ITEMS: usize = 4096;
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const ACCEPT_POLL: Duration = Duration::from_millis(25);
const DEFAULT_SOCKET: &str = "/run/mackesd/store-writer.sock";

static SERVE_SOCKET: OnceLock<PathBuf> = OnceLock::new();

/// Configure this process as one member of the split `serve` runtime.
///
/// This may be called only once. Afterwards every admitted canonical helper
/// mutation uses this socket. Ordinary opens remain compatible while the
/// checked residual direct-SQL inventory is migrated incrementally.
pub fn configure_serve_process(socket: Option<PathBuf>) -> Result<()> {
    let socket = socket
        .or_else(|| std::env::var_os("MACKESD_STORE_WRITER_SOCKET").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET));
    SERVE_SOCKET
        .set(socket)
        .map_err(|_| anyhow::anyhow!("mackesd store serve-process access already configured"))
}

#[must_use]
pub fn serve_process_is_configured() -> bool {
    SERVE_SOCKET.get().is_some()
}

pub fn configured_socket() -> Result<&'static Path> {
    SERVE_SOCKET
        .get()
        .map(PathBuf::as_path)
        .context("mackesd store serve-process access is not configured")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum WriteOp {
    LoadClockAuthority {
        node_id: String,
    },
    LoadPendingClockAudio {
        node_id: String,
    },
    AcknowledgeClockAudio {
        node_id: String,
        request_id: String,
        occurrence_id: String,
        global_event_id: String,
        occurrence_generation: u64,
        acknowledged_at_ms: i64,
    },
    CommitClockAuthority {
        node_id: String,
        expected_revision: u64,
        new_revision: u64,
        request_id: Option<String>,
        request_fingerprint: Option<String>,
        action_cursor: Option<String>,
        snapshot_json: String,
        updated_at_ms: i64,
        audio_requests: Vec<ClockAudioOutboxWrite>,
    },
    AppendReconcileEvents {
        events: Vec<crate::events::Event>,
    },
    AppendEventRecord {
        event_id: u64,
        kind: String,
        node_id: String,
        timestamp_ms: i64,
        detail: serde_json::Value,
    },
    CreateApprovedRevision {
        target_revision_id: i64,
        author: String,
        message: String,
        created_at: String,
    },
    RecordFleetPush {
        key: String,
        value_json: String,
        peers: Vec<String>,
        author: String,
    },
    InsertEvent {
        kind: String,
        actor: String,
        payload_json: String,
    },
    RollbackToRevision {
        target_id: String,
        new_id: String,
        author: String,
    },
    SetNodeRole {
        node_id: String,
        role: String,
    },
    SetNodeHealth {
        node_id: String,
        health: String,
    },
    SetNodeVersion {
        name: String,
        version: Option<String>,
    },
    RefreshNodeCredentials {
        node_id: String,
        new_public_key: String,
    },
    UpsertNode {
        node_id: String,
        name: String,
        public_key: String,
        region: Option<String>,
    },
    MintCa {
        mesh_id: String,
        ca_cert_pem: String,
    },
    SeedLighthouseCa {
        mesh_id: String,
        epoch: i64,
        ca_cert_pem: String,
    },
    UpsertPeerCert {
        mesh_id: String,
        expected_epoch: i64,
        peer: CaPeerCertWrite,
    },
    RevokePeerCert {
        node_id: String,
        revoked_at: i64,
    },
    RestoreCaBackup {
        mesh_id: String,
        ca_certs: Vec<CaCertWrite>,
        peer_certs: Vec<CaPeerCertWrite>,
    },
    RotateCa {
        mesh_id: String,
        expected_active_epoch: Option<i64>,
        new_epoch: i64,
        ca_cert_pem: String,
        peer_certs: Vec<CaPeerCertWrite>,
    },
    #[cfg(test)]
    InsertDesiredConfigFixture {
        author: String,
        message: String,
        spec: serde_json::Value,
        state: String,
        created_at: String,
        applied_at: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaCertWrite {
    pub epoch: i64,
    pub ca_cert_pem: String,
    pub created_at: i64,
    pub retired_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaPeerCertWrite {
    pub node_id: String,
    pub epoch: i64,
    pub cert_pem: String,
    pub overlay_ip: String,
    pub public_key_pem: Option<String>,
    pub created_at: Option<i64>,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockAuthorityRecord {
    pub node_id: String,
    pub revision: u64,
    pub snapshot_json: String,
    pub action_cursor: Option<String>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockAudioOutboxWrite {
    pub request_id: String,
    pub occurrence_id: String,
    pub global_event_id: String,
    pub occurrence_generation: u64,
    pub request_json: String,
    pub created_at_ms: i64,
}

pub type ClockAudioOutboxRecord = ClockAudioOutboxWrite;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteRequest {
    schema_version: u16,
    operation: WriteOp,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
pub enum WriteResponse {
    RowId(i64),
    Count(usize),
    Changed(bool),
    ClockAuthority(Option<ClockAuthorityRecord>),
    ClockAudioOutbox(Vec<ClockAudioOutboxRecord>),
    Error(String),
}

impl WriteResponse {
    pub fn into_row_id(self) -> Result<i64> {
        match self {
            Self::RowId(value) => Ok(value),
            Self::Error(error) => bail!(error),
            other => bail!("store writer returned wrong response: {other:?}"),
        }
    }

    pub fn into_count(self) -> Result<usize> {
        match self {
            Self::Count(value) => Ok(value),
            Self::Error(error) => bail!(error),
            other => bail!("store writer returned wrong response: {other:?}"),
        }
    }

    pub fn into_changed(self) -> Result<bool> {
        match self {
            Self::Changed(value) => Ok(value),
            Self::Error(error) => bail!(error),
            other => bail!("store writer returned wrong response: {other:?}"),
        }
    }

    pub fn into_clock_authority(self) -> Result<Option<ClockAuthorityRecord>> {
        match self {
            Self::ClockAuthority(value) => Ok(value),
            Self::Error(error) => bail!(error),
            other => bail!("store writer returned wrong response: {other:?}"),
        }
    }

    pub fn into_clock_audio_outbox(self) -> Result<Vec<ClockAudioOutboxRecord>> {
        match self {
            Self::ClockAudioOutbox(value) => Ok(value),
            Self::Error(error) => bail!(error),
            other => bail!("store writer returned wrong response: {other:?}"),
        }
    }
}

pub fn request_if_serving(operation: WriteOp) -> Result<Option<WriteResponse>> {
    let Some(socket) = SERVE_SOCKET.get() else {
        return Ok(None);
    };
    request(socket, operation).map(Some)
}

/// Send a typed mutation to the split-process owner, or execute it through the
/// same finite dispatcher when this process is the standalone store owner.
pub fn request_or_execute(conn: &Connection, operation: WriteOp) -> Result<WriteResponse> {
    match request_if_serving(operation.clone())? {
        Some(response) => Ok(response),
        None => execute(conn, operation),
    }
}

fn request(socket: &Path, operation: WriteOp) -> Result<WriteResponse> {
    let mut stream = connect_bounded(socket, IO_TIMEOUT)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let payload = serde_json::to_vec(&WriteRequest {
        schema_version: SCHEMA_VERSION,
        operation,
    })?;
    write_frame(&mut stream, &payload)?;
    let response = read_frame(&mut stream)?;
    serde_json::from_slice(&response).context("decoding SQLite writer response")
}

fn connect_bounded(socket: &Path, timeout: Duration) -> Result<UnixStream> {
    let deadline = Instant::now() + timeout;
    loop {
        match UnixStream::connect(socket) {
            Ok(stream) => return Ok(stream),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) && Instant::now() < deadline =>
            {
                std::thread::sleep(ACCEPT_POLL);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "SQLite writer {} unavailable after {} ms",
                        socket.display(),
                        timeout.as_millis()
                    )
                });
            }
        }
    }
}

pub struct WriterServer {
    socket: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WriterServer {
    pub fn join(mut self) -> Result<()> {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("SQLite writer thread panicked"))?;
        }
        Ok(())
    }
}

impl Drop for WriterServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Bind and migrate synchronously, then run the sole persistent writable
/// connection on a dedicated thread. Returning success means clients may
/// safely connect; startup failures remain fatal to the control group.
pub fn start(db_path: &Path, socket: &Path, shutdown: Arc<AtomicBool>) -> Result<WriterServer> {
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating SQLite writer dir {}", parent.display()))?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    if socket.exists() {
        match UnixStream::connect(socket) {
            Ok(_) => bail!("SQLite writer socket {} is already live", socket.display()),
            Err(_) => std::fs::remove_file(socket)
                .with_context(|| format!("removing stale writer socket {}", socket.display()))?,
        }
    }
    let listener = UnixListener::bind(socket)
        .with_context(|| format!("binding SQLite writer socket {}", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let connection = Connection::open(db_path)
        .with_context(|| format!("opening SQLite writer db {}", db_path.display()))?;
    super::migrate(&connection)?;

    let socket_path = socket.to_owned();
    let cleanup_path = socket_path.clone();
    let thread_shutdown = Arc::clone(&shutdown);
    let thread = std::thread::Builder::new()
        .name("sqlite-writer".into())
        .spawn(move || {
            while !thread_shutdown.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                        let response = match read_frame(&mut stream)
                            .and_then(|bytes| decode_and_execute(&connection, &bytes))
                        {
                            Ok(response) => response,
                            Err(error) => WriteResponse::Error(error.to_string()),
                        };
                        if let Ok(bytes) = serde_json::to_vec(&response) {
                            let _ = write_frame(&mut stream, &bytes);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(ACCEPT_POLL);
                    }
                    Err(error) => {
                        tracing::error!(%error, "SQLite writer accept failed");
                        std::thread::sleep(ACCEPT_POLL);
                    }
                }
            }
            drop(connection);
            let _ = std::fs::remove_file(cleanup_path);
        })?;
    Ok(WriterServer {
        socket: socket_path,
        shutdown,
        thread: Some(thread),
    })
}

fn decode_and_execute(conn: &Connection, payload: &[u8]) -> Result<WriteResponse> {
    let payload = std::str::from_utf8(payload).context("write request is not UTF-8")?;
    mackes_mesh_types::workloads::reject_duplicate_json_keys(payload)
        .context("write request contains duplicate JSON keys")?;
    let request: WriteRequest = serde_json::from_str(payload)
        .map_err(|error| anyhow::anyhow!("decoding write request: {error}"))?;
    if request.schema_version != SCHEMA_VERSION {
        bail!(
            "unsupported SQLite writer schema {}; expected {}",
            request.schema_version,
            SCHEMA_VERSION
        );
    }
    execute(conn, request.operation)
}

/// Install the deterministic Bus failure used by the host-state compensation
/// proof. Keeping this test-only mutation inside the store fixture boundary
/// leaves production workers unable to open writable SQLite connections.
#[cfg(test)]
pub(crate) fn install_reject_host_apply_fixture(bus_index: &Path) -> Result<()> {
    let connection = Connection::open(bus_index)
        .with_context(|| format!("opening Bus fixture {}", bus_index.display()))?;
    connection.execute_batch(
        "CREATE TRIGGER reject_host_apply BEFORE INSERT ON messages
         WHEN NEW.topic = 'action/host/local/apply'
         BEGIN SELECT RAISE(FAIL, 'host apply rejected'); END;",
    )?;
    Ok(())
}

fn execute(conn: &Connection, operation: WriteOp) -> Result<WriteResponse> {
    match operation {
        WriteOp::LoadClockAuthority { node_id } => load_clock_authority(conn, &node_id),
        WriteOp::LoadPendingClockAudio { node_id } => load_pending_clock_audio(conn, &node_id),
        WriteOp::AcknowledgeClockAudio {
            node_id,
            request_id,
            occurrence_id,
            global_event_id,
            occurrence_generation,
            acknowledged_at_ms,
        } => acknowledge_clock_audio(
            conn,
            &node_id,
            &request_id,
            &occurrence_id,
            &global_event_id,
            occurrence_generation,
            acknowledged_at_ms,
        ),
        WriteOp::CommitClockAuthority {
            node_id,
            expected_revision,
            new_revision,
            request_id,
            request_fingerprint,
            action_cursor,
            snapshot_json,
            updated_at_ms,
            audio_requests,
        } => commit_clock_authority(
            conn,
            &node_id,
            expected_revision,
            new_revision,
            request_id.as_deref(),
            request_fingerprint.as_deref(),
            action_cursor.as_deref(),
            &snapshot_json,
            updated_at_ms,
            &audio_requests,
        ),
        WriteOp::AppendReconcileEvents { events } => append_reconcile_events(conn, &events),
        WriteOp::AppendEventRecord {
            event_id,
            kind,
            node_id,
            timestamp_ms,
            detail,
        } => append_event_record(conn, event_id, &kind, &node_id, timestamp_ms, &detail),
        WriteOp::CreateApprovedRevision {
            target_revision_id,
            author,
            message,
            created_at,
        } => create_approved_revision(conn, target_revision_id, &author, &message, &created_at),
        WriteOp::RecordFleetPush {
            key,
            value_json,
            peers,
            author,
        } => record_fleet_push(conn, &key, &value_json, &peers, &author),
        WriteOp::InsertEvent {
            kind,
            actor,
            payload_json,
        } => {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| {
                let prev_hash_hex: String = conn
                    .query_row(
                        "SELECT hash FROM events ORDER BY seq DESC LIMIT 1",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or_default();
                let prev_bytes = super::decode_sha256_hex(&prev_hash_hex).unwrap_or([0; 32]);
                let now = chrono::Utc::now();
                let hash = crate::audit::next_hash(
                    &prev_bytes,
                    payload_json.as_bytes(),
                    now.timestamp_millis(),
                );
                let hash_hex = super::encode_sha256_hex(&hash);
                conn.execute(
                    "INSERT INTO events (prev_hash, hash, kind, actor, payload_json, created_at) VALUES (?, ?, ?, ?, ?, ?)",
                    (&prev_hash_hex, &hash_hex, kind, actor, payload_json, now.to_rfc3339()),
                )?;
                Ok(WriteResponse::RowId(conn.last_insert_rowid()))
            })();
            finish_transaction(conn, result)
        }
        WriteOp::RollbackToRevision {
            target_id,
            new_id,
            author,
        } => {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| {
                let payload: String = conn.query_row(
                    "SELECT payload_json FROM applied_changes WHERE revision_id = ? LIMIT 1",
                    [&target_id],
                    |row| row.get(0),
                )?;
                let now = chrono::Utc::now().to_rfc3339();
                let count = conn.execute(
                    "INSERT INTO applied_changes (revision_id, author, summary, created_at, applied_at, payload_json) VALUES (?, ?, ?, ?, ?, ?)",
                    (&new_id, &author, format!("Rollback to {target_id}"), &now, &now, payload),
                )?;
                Ok(WriteResponse::Count(count))
            })();
            finish_transaction(conn, result)
        }
        WriteOp::SetNodeRole { node_id, role } => {
            validate_identity(&node_id, "node id")?;
            if !matches!(
                role.as_str(),
                "host" | "peer" | "observer" | "decommissioned"
            ) {
                bail!("invalid node role");
            }
            Ok(WriteResponse::Count(conn.execute(
                "UPDATE nodes SET role = ? WHERE node_id = ?",
                (role, node_id),
            )?))
        }
        WriteOp::SetNodeHealth { node_id, health } => {
            validate_identity(&node_id, "node id")?;
            if !matches!(
                health.as_str(),
                "healthy" | "degraded" | "unreachable" | "unknown"
            ) {
                bail!("invalid node health");
            }
            let prior: Option<String> = conn
                .query_row(
                    "SELECT health FROM nodes WHERE node_id = ?",
                    [&node_id],
                    |row| row.get(0),
                )
                .optional()?;
            if prior.as_deref().is_none_or(|prior| prior == health) {
                return Ok(WriteResponse::Changed(false));
            }
            Ok(WriteResponse::Changed(
                conn.execute(
                    "UPDATE nodes SET health = ? WHERE node_id = ?",
                    (health, node_id),
                )? > 0,
            ))
        }
        WriteOp::SetNodeVersion { name, version } => {
            validate_bounded_single_line(&name, "node name", MAX_NAME_BYTES)?;
            if let Some(version) = version.as_deref() {
                validate_bounded_single_line(version, "node version", MAX_NAME_BYTES)?;
            }
            Ok(WriteResponse::Changed(
                conn.execute(
                    "UPDATE nodes SET mde_version = ? WHERE name = ?",
                    (version, name),
                )? > 0,
            ))
        }
        WriteOp::RefreshNodeCredentials {
            node_id,
            new_public_key,
        } => {
            validate_identity(&node_id, "node id")?;
            validate_bounded_text(&new_public_key, "node public key", MAX_PUBLIC_KEY_BYTES)?;
            Ok(WriteResponse::Count(conn.execute(
                "UPDATE nodes SET public_key = ?, enrolled_at = ? WHERE node_id = ?",
                (new_public_key, chrono::Utc::now().to_rfc3339(), node_id),
            )?))
        }
        WriteOp::UpsertNode {
            node_id,
            name,
            public_key,
            region,
        } => {
            validate_identity(&node_id, "node id")?;
            validate_bounded_single_line(&name, "node name", MAX_NAME_BYTES)?;
            validate_bounded_text(&public_key, "node public key", MAX_PUBLIC_KEY_BYTES)?;
            if let Some(region) = region.as_deref() {
                validate_bounded_single_line(region, "node region", MAX_REGION_BYTES)?;
            }
            Ok(WriteResponse::Count(conn.execute(
                "INSERT INTO nodes (node_id, name, public_key, enrolled_at, region) VALUES (?, ?, ?, ?, ?) ON CONFLICT(node_id) DO UPDATE SET name = excluded.name, public_key = excluded.public_key, region = excluded.region",
                (node_id, name, public_key, chrono::Utc::now().to_rfc3339(), region),
            )?))
        }
        WriteOp::MintCa {
            mesh_id,
            ca_cert_pem,
        } => mint_ca(conn, &mesh_id, &ca_cert_pem),
        WriteOp::SeedLighthouseCa {
            mesh_id,
            epoch,
            ca_cert_pem,
        } => seed_lighthouse_ca(conn, &mesh_id, epoch, &ca_cert_pem),
        WriteOp::UpsertPeerCert {
            mesh_id,
            expected_epoch,
            peer,
        } => upsert_peer_cert(conn, &mesh_id, expected_epoch, &peer),
        WriteOp::RevokePeerCert {
            node_id,
            revoked_at,
        } => revoke_peer_cert(conn, &node_id, revoked_at),
        WriteOp::RestoreCaBackup {
            mesh_id,
            ca_certs,
            peer_certs,
        } => restore_ca_backup(conn, &mesh_id, &ca_certs, &peer_certs),
        WriteOp::RotateCa {
            mesh_id,
            expected_active_epoch,
            new_epoch,
            ca_cert_pem,
            peer_certs,
        } => rotate_ca(
            conn,
            &mesh_id,
            expected_active_epoch,
            new_epoch,
            &ca_cert_pem,
            &peer_certs,
        ),
        #[cfg(test)]
        WriteOp::InsertDesiredConfigFixture {
            author,
            message,
            spec,
            state,
            created_at,
            applied_at,
        } => insert_desired_config_fixture_inner(
            conn,
            &author,
            &message,
            &spec,
            &state,
            &created_at,
            applied_at.as_deref(),
        ),
    }
}

fn load_clock_authority(conn: &Connection, node_id: &str) -> Result<WriteResponse> {
    validate_identity(node_id, "Clock node id")?;
    let row = conn
        .query_row(
            "SELECT revision, snapshot_json, action_cursor, updated_at_ms FROM clock_authority WHERE node_id = ?1",
            [node_id],
            |row| {
                let revision = row.get::<_, i64>(0)?;
                Ok((
                    revision,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?;
    let record = row
        .map(|(revision, snapshot_json, action_cursor, updated_at_ms)| {
            Ok::<ClockAuthorityRecord, anyhow::Error>(ClockAuthorityRecord {
                node_id: node_id.to_owned(),
                revision: u64::try_from(revision).context("Clock revision is negative")?,
                snapshot_json,
                action_cursor,
                updated_at_ms,
            })
        })
        .transpose()?;
    Ok(WriteResponse::ClockAuthority(record))
}

#[allow(clippy::too_many_arguments)]
fn commit_clock_authority(
    conn: &Connection,
    node_id: &str,
    expected_revision: u64,
    new_revision: u64,
    request_id: Option<&str>,
    request_fingerprint: Option<&str>,
    action_cursor: Option<&str>,
    snapshot_json: &str,
    updated_at_ms: i64,
    audio_requests: &[ClockAudioOutboxWrite],
) -> Result<WriteResponse> {
    validate_identity(node_id, "Clock node id")?;
    if let Some(request_id) = request_id {
        validate_identity(request_id, "Clock request id")?;
    }
    match (request_id, request_fingerprint) {
        (Some(_), Some(fingerprint))
            if fingerprint.len() == 64
                && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) => {}
        (None, None) => {}
        _ => bail!("Clock request identity/fingerprint mismatch"),
    }
    if let Some(cursor) = action_cursor {
        validate_bounded_single_line(cursor, "Clock action cursor", MAX_IDENTITY_BYTES)?;
    }
    if snapshot_json.len() > MAX_CLOCK_SNAPSHOT_BYTES {
        bail!("Clock snapshot exceeds the finite writer envelope");
    }
    if updated_at_ms <= 0
        || new_revision == 0
        || new_revision > i64::MAX as u64
        || expected_revision > i64::MAX as u64
        || !(new_revision == expected_revision
            || expected_revision.checked_add(1) == Some(new_revision))
    {
        bail!("invalid Clock revision or timestamp");
    }
    mackes_mesh_types::workloads::reject_duplicate_json_keys(snapshot_json)
        .context("Clock snapshot contains duplicate JSON keys")?;
    let snapshot: mackes_mesh_types::clock::ClockSnapshotV1 =
        serde_json::from_str(snapshot_json).context("Clock snapshot is not v1 JSON")?;
    if snapshot.node_id != node_id
        || snapshot.revision != new_revision
        || snapshot.produced_at_utc_ms != updated_at_ms
    {
        bail!("Clock snapshot identity/revision/timestamp mismatch");
    }
    let zone_exists = |zone: &str| {
        !zone.starts_with('/')
            && !zone.contains("..")
            && Path::new("/usr/share/zoneinfo").join(zone).is_file()
    };
    snapshot
        .validate_at(&mackes_mesh_types::clock::ClockValidationContext {
            wall_utc_ms: updated_at_ms,
            monotonic_ms: 1,
            zone_exists: &zone_exists,
        })
        .context("Clock snapshot failed contract validation")?;
    validate_clock_audio_writes(node_id, audio_requests, updated_at_ms)?;

    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        if let Some(request_id) = request_id {
            let seen_fingerprint = conn
                .query_row(
                    "SELECT request_fingerprint FROM clock_request_ledger WHERE node_id = ?1 AND request_id = ?2",
                    (node_id, request_id),
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?;
            if let Some(seen_fingerprint) = seen_fingerprint {
                // Both exact replays and conflicting request-id reuse are
                // deterministic no-ops. Advance only the consumed Bus cursor;
                // never apply the caller's candidate snapshot in either case.
                // A NULL fingerprint is a pre-v16 ledger row and therefore
                // cannot prove an exact replay, so it also fails closed.
                let _same_admitted_command = seen_fingerprint
                    .as_deref()
                    .is_some_and(|seen| Some(seen) == request_fingerprint);
                conn.execute(
                    "UPDATE clock_authority SET action_cursor = ?2 WHERE node_id = ?1",
                    (node_id, action_cursor),
                )?;
                return Ok(WriteResponse::Changed(false));
            }
        }
        let current: Option<i64> = conn
            .query_row(
                "SELECT revision FROM clock_authority WHERE node_id = ?1",
                [node_id],
                |row| row.get(0),
            )
            .optional()?;
        let current = current.map_or(0, |value| value);
        if current != i64::try_from(expected_revision)? {
            bail!("stale Clock authority revision");
        }
        let mut pending_count: usize = conn.query_row(
            "SELECT COUNT(*) FROM clock_audio_outbox WHERE node_id = ?1 AND acknowledged_at_ms IS NULL",
            [node_id],
            |row| row.get(0),
        )?;
        for request in audio_requests {
            let already_present: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM clock_audio_outbox WHERE node_id = ?1 AND request_id = ?2)",
                (node_id, &request.request_id),
                |row| row.get(0),
            )?;
            if !already_present {
                pending_count = pending_count
                    .checked_add(1)
                    .context("Clock audio outbox count overflow")?;
            }
        }
        if pending_count > MAX_CLOCK_AUDIO_OUTBOX_ROWS {
            bail!("Clock audio outbox capacity exhausted");
        }
        conn.execute(
            "INSERT INTO clock_authority (node_id, revision, snapshot_json, action_cursor, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(node_id) DO UPDATE SET revision = excluded.revision, snapshot_json = excluded.snapshot_json, action_cursor = excluded.action_cursor, updated_at_ms = excluded.updated_at_ms",
            (node_id, i64::try_from(new_revision)?, snapshot_json, action_cursor, updated_at_ms),
        )?;
        if let Some(request_id) = request_id {
            conn.execute(
                "INSERT INTO clock_request_ledger (node_id, request_id, revision, applied_at_ms, request_fingerprint) VALUES (?1, ?2, ?3, ?4, ?5)",
                (node_id, request_id, i64::try_from(new_revision)?, updated_at_ms, request_fingerprint),
            )?;
        }
        for request in audio_requests {
            conn.execute(
                "INSERT OR IGNORE INTO clock_audio_outbox (node_id, request_id, occurrence_id, global_event_id, occurrence_generation, request_json, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                (
                    node_id,
                    &request.request_id,
                    &request.occurrence_id,
                    &request.global_event_id,
                    i64::try_from(request.occurrence_generation)?,
                    &request.request_json,
                    request.created_at_ms,
                ),
            )?;
        }
        Ok(WriteResponse::Changed(true))
    })();
    finish_transaction(conn, result)
}

fn validate_clock_audio_writes(
    node_id: &str,
    requests: &[ClockAudioOutboxWrite],
    now_ms: i64,
) -> Result<()> {
    if requests.len() > MAX_CLOCK_AUDIO_OUTBOX_ROWS {
        bail!("Clock audio outbox batch exceeds its finite cap");
    }
    let mut ids = std::collections::BTreeSet::new();
    for request in requests {
        validate_identity(&request.request_id, "Clock audio request id")?;
        validate_identity(&request.occurrence_id, "Clock audio occurrence id")?;
        validate_identity(&request.global_event_id, "Clock audio global event id")?;
        if request.occurrence_generation == 0
            || request.occurrence_generation > i64::MAX as u64
            || request.created_at_ms != now_ms
            || !ids.insert(&request.request_id)
        {
            bail!("invalid or duplicate Clock audio outbox row");
        }
        mackes_mesh_types::workloads::reject_duplicate_json_keys(&request.request_json)
            .context("Clock audio request contains duplicate JSON keys")?;
        let typed = mackes_mesh_types::clock::ClockAudioRequestV1::from_json_at(
            request.request_json.as_bytes(),
            now_ms,
        )
        .context("Clock audio request failed contract validation")?;
        if typed.request_id != request.request_id
            || typed.occurrence_id != request.occurrence_id
            || typed.global_event_id != request.global_event_id
            || typed.occurrence_generation != request.occurrence_generation
            || typed.music_auth.is_some()
        {
            bail!("Clock audio outbox identity or authorization mismatch");
        }
    }
    validate_identity(node_id, "Clock audio node id")
}

fn load_pending_clock_audio(conn: &Connection, node_id: &str) -> Result<WriteResponse> {
    validate_identity(node_id, "Clock audio node id")?;
    let mut statement = conn.prepare(
        "SELECT request_id, occurrence_id, global_event_id, occurrence_generation, request_json, created_at_ms FROM clock_audio_outbox WHERE node_id = ?1 AND acknowledged_at_ms IS NULL ORDER BY created_at_ms, request_id LIMIT ?2",
    )?;
    let rows = statement.query_map((node_id, MAX_CLOCK_AUDIO_OUTBOX_ROWS as i64), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
        ))
    })?;
    let mut records = Vec::new();
    for row in rows {
        let (request_id, occurrence_id, global_event_id, generation, request_json, created_at_ms) =
            row?;
        records.push(ClockAudioOutboxRecord {
            request_id,
            occurrence_id,
            global_event_id,
            occurrence_generation: u64::try_from(generation)
                .context("Clock audio generation is negative")?,
            request_json,
            created_at_ms,
        });
    }
    Ok(WriteResponse::ClockAudioOutbox(records))
}

#[allow(clippy::too_many_arguments)]
fn acknowledge_clock_audio(
    conn: &Connection,
    node_id: &str,
    request_id: &str,
    occurrence_id: &str,
    global_event_id: &str,
    occurrence_generation: u64,
    acknowledged_at_ms: i64,
) -> Result<WriteResponse> {
    validate_identity(node_id, "Clock audio node id")?;
    validate_identity(request_id, "Clock audio request id")?;
    validate_identity(occurrence_id, "Clock audio occurrence id")?;
    validate_identity(global_event_id, "Clock audio global event id")?;
    if occurrence_generation == 0
        || occurrence_generation > i64::MAX as u64
        || acknowledged_at_ms <= 0
    {
        bail!("invalid Clock audio acknowledgement");
    }
    Ok(WriteResponse::Changed(
        conn.execute(
            "DELETE FROM clock_audio_outbox WHERE node_id = ?1 AND request_id = ?2 AND occurrence_id = ?3 AND global_event_id = ?4 AND occurrence_generation = ?5 AND created_at_ms <= ?6 AND acknowledged_at_ms IS NULL",
            (
                node_id,
                request_id,
                occurrence_id,
                global_event_id,
                i64::try_from(occurrence_generation)?,
                acknowledged_at_ms,
            ),
        )? > 0,
    ))
}

fn append_reconcile_events(
    conn: &Connection,
    events: &[crate::events::Event],
) -> Result<WriteResponse> {
    if events.is_empty() || events.len() > MAX_RECONCILE_EVENTS {
        bail!("reconcile event batch must contain 1..={MAX_RECONCILE_EVENTS} rows");
    }

    let mut durable_rows = Vec::with_capacity(events.len());
    let mut unique_payloads = std::collections::BTreeSet::new();
    for event in events {
        if event.kind != crate::events::EventKind::Reconcile {
            bail!("reconcile event batch contains a non-reconcile event");
        }
        validate_identity(&event.node_id, "reconcile event node id")?;
        if event.timestamp_ms < 0 {
            bail!("reconcile event timestamp must be non-negative");
        }
        if event.event_id < u64::try_from(event.timestamp_ms)? {
            bail!("reconcile event id precedes its timestamp");
        }
        if !event.detail.is_object() {
            bail!("reconcile event detail must be a JSON object");
        }
        validate_json_shape(&event.detail, 0)?;
        let payload = serde_json::to_string(event).context("serializing reconcile event")?;
        if payload.len() > MAX_EVENT_DETAIL_BYTES {
            bail!("reconcile event payload exceeds the finite writer envelope");
        }
        if !unique_payloads.insert(payload.clone()) {
            bail!("reconcile event batch contains duplicate rows");
        }
        let created_at = chrono::DateTime::from_timestamp_millis(event.timestamp_ms)
            .context("reconcile event timestamp is outside the supported range")?
            .to_rfc3339();
        durable_rows.push((event, payload, created_at));
    }

    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        let mut existing_rows = 0_usize;
        for (event, payload, created_at) in &durable_rows {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM events WHERE kind = 'reconcile' AND actor = ?1 AND payload_json = ?2 AND created_at = ?3",
                (&event.node_id, payload, created_at),
                |row| row.get(0),
            )?;
            match count {
                0 => {}
                1 => existing_rows += 1,
                _ => bail!("reconcile event replay found duplicate durable rows"),
            }
        }
        if existing_rows == durable_rows.len() {
            return Ok(WriteResponse::Count(0));
        }
        if existing_rows != 0 {
            bail!("reconcile event replay found an incomplete durable transaction");
        }

        let prev_hash_hex: String = conn
            .query_row(
                "SELECT hash FROM events ORDER BY seq DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or_default();
        let mut prev_hash = super::decode_sha256_hex(&prev_hash_hex).unwrap_or([0; 32]);
        for (event, payload, created_at) in &durable_rows {
            let hash = crate::audit::next_hash(&prev_hash, payload.as_bytes(), event.timestamp_ms);
            conn.execute(
                "INSERT INTO events (prev_hash, hash, kind, actor, payload_json, created_at) VALUES (?1, ?2, 'reconcile', ?3, ?4, ?5)",
                (
                    super::encode_sha256_hex(&prev_hash),
                    super::encode_sha256_hex(&hash),
                    &event.node_id,
                    payload,
                    created_at,
                ),
            )?;
            prev_hash = hash;
        }
        Ok(WriteResponse::Count(durable_rows.len()))
    })();
    finish_transaction(conn, result)
}

#[cfg(test)]
fn insert_desired_config_fixture_inner(
    conn: &Connection,
    author: &str,
    message: &str,
    spec: &serde_json::Value,
    state: &str,
    created_at: &str,
    applied_at: Option<&str>,
) -> Result<WriteResponse> {
    validate_bounded_single_line(author, "fixture author", MAX_IDENTITY_BYTES)?;
    validate_bounded_single_line(message, "fixture message", MAX_SUMMARY_BYTES)?;
    if !matches!(state, "draft" | "validated" | "applied" | "verified") {
        bail!("invalid desired-config fixture state");
    }
    validate_bounded_single_line(created_at, "fixture creation timestamp", MAX_NAME_BYTES)?;
    if let Some(applied_at) = applied_at {
        validate_bounded_single_line(applied_at, "fixture applied timestamp", MAX_NAME_BYTES)?;
    }
    validate_json_shape(spec, 0)?;
    let spec_json = serde_json::to_string(spec)?;
    if spec_json.len() > MAX_REVISION_SPEC_BYTES {
        bail!("desired-config fixture exceeds the finite writer envelope");
    }

    let existing: Option<i64> = conn
        .query_row(
            "SELECT revision_id FROM desired_config WHERE author = ?1 AND message = ?2 AND spec_json = ?3 AND state = ?4 AND created_at = ?5 AND applied_at IS ?6 LIMIT 1",
            (author, message, &spec_json, state, created_at, applied_at),
            |row| row.get(0),
        )
        .optional()?;
    if let Some(revision_id) = existing {
        return Ok(WriteResponse::RowId(revision_id));
    }
    conn.execute(
        "INSERT INTO desired_config (author, message, spec_json, state, created_at, applied_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (author, message, spec_json, state, created_at, applied_at),
    )?;
    Ok(WriteResponse::RowId(conn.last_insert_rowid()))
}

#[cfg(test)]
pub fn insert_desired_config_fixture(
    conn: &Connection,
    spec: serde_json::Value,
    state: &str,
) -> Result<i64> {
    request_or_execute(
        conn,
        WriteOp::InsertDesiredConfigFixture {
            author: "tester".to_owned(),
            message: "seed".to_owned(),
            spec,
            state: state.to_owned(),
            created_at: "2026-05-19T00:00:00Z".to_owned(),
            applied_at: matches!(state, "applied" | "verified")
                .then(|| "2026-05-19T00:00:00Z".to_owned()),
        },
    )?
    .into_row_id()
}

#[derive(Serialize)]
struct EventRecord<'a> {
    event_id: u64,
    kind: &'a str,
    node_id: &'a str,
    timestamp_ms: i64,
    detail: &'a serde_json::Value,
}

fn append_event_record(
    conn: &Connection,
    event_id: u64,
    kind: &str,
    node_id: &str,
    timestamp_ms: i64,
    detail: &serde_json::Value,
) -> Result<WriteResponse> {
    validate_identity(node_id, "event node id")?;
    if !matches!(
        kind,
        "config_change" | "auth" | "lifecycle" | "reconcile" | "admin_action"
    ) {
        bail!("invalid event kind");
    }
    if timestamp_ms < 0 || u64::try_from(timestamp_ms).ok() != Some(event_id) {
        bail!("event id must exactly match its non-negative millisecond timestamp");
    }
    if !detail.is_object() {
        bail!("event detail must be a JSON object");
    }
    validate_json_shape(detail, 0)?;
    let payload = serde_json::to_string(&EventRecord {
        event_id,
        kind,
        node_id,
        timestamp_ms,
        detail,
    })
    .context("serializing typed event payload")?;
    if payload.len() > MAX_EVENT_DETAIL_BYTES {
        bail!("event payload exceeds the finite writer envelope");
    }
    let created_at = chrono::DateTime::from_timestamp_millis(timestamp_ms)
        .context("event timestamp is outside the supported range")?
        .to_rfc3339();

    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT seq FROM events WHERE payload_json = ?1 AND kind = ?2 AND actor = ?3 AND created_at = ?4 LIMIT 1",
                (&payload, kind, node_id, &created_at),
                |row| row.get(0),
            )
            .optional()?;
        if let Some(seq) = existing {
            return Ok(WriteResponse::RowId(seq));
        }
        let prev_hash_hex: String = conn
            .query_row(
                "SELECT hash FROM events ORDER BY seq DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or_default();
        let prev_hash = super::decode_sha256_hex(&prev_hash_hex).unwrap_or([0_u8; 32]);
        let hash = crate::audit::next_hash(&prev_hash, payload.as_bytes(), timestamp_ms);
        conn.execute(
            "INSERT INTO events (prev_hash, hash, kind, actor, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                super::encode_sha256_hex(&prev_hash),
                super::encode_sha256_hex(&hash),
                kind,
                node_id,
                payload,
                created_at,
            ),
        )?;
        Ok(WriteResponse::RowId(conn.last_insert_rowid()))
    })();
    finish_transaction(conn, result)
}

fn create_approved_revision(
    conn: &Connection,
    target_revision_id: i64,
    author: &str,
    message: &str,
    created_at: &str,
) -> Result<WriteResponse> {
    if target_revision_id <= 0 {
        bail!("target revision id must be positive");
    }
    validate_bounded_single_line(author, "revision author", MAX_IDENTITY_BYTES)?;
    validate_bounded_single_line(message, "revision message", MAX_SUMMARY_BYTES)?;
    let parsed_at = chrono::DateTime::parse_from_rfc3339(created_at)
        .context("revision creation timestamp is not RFC3339")?;
    if parsed_at.timestamp_millis() < 0 {
        bail!("revision creation timestamp predates the supported epoch");
    }

    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        let payload: String = conn
            .query_row(
                "SELECT spec_json FROM desired_config WHERE revision_id = ?1",
                [target_revision_id],
                |row| row.get(0),
            )
            .with_context(|| format!("loading desired revision {target_revision_id}"))?;
        if payload.len() > MAX_REVISION_SPEC_BYTES {
            bail!("desired revision payload exceeds the finite writer envelope");
        }
        mackes_mesh_types::workloads::reject_duplicate_json_keys(&payload)
            .context("desired revision payload contains duplicate JSON keys")?;
        let parsed_payload: serde_json::Value =
            serde_json::from_str(&payload).context("desired revision payload is not JSON")?;
        validate_json_shape(&parsed_payload, 0)?;
        let existing: Option<i64> = conn
            .query_row(
                "SELECT revision_id FROM desired_config WHERE author = ?1 AND message = ?2 AND spec_json = ?3 AND state = 'approved' AND created_at = ?4 LIMIT 1",
                (author, message, &payload, created_at),
                |row| row.get(0),
            )
            .optional()?;
        if let Some(revision_id) = existing {
            return Ok(WriteResponse::RowId(revision_id));
        }
        conn.execute(
            "INSERT INTO desired_config (author, message, spec_json, state, created_at) VALUES (?1, ?2, ?3, 'approved', ?4)",
            (author, message, payload, created_at),
        )?;
        Ok(WriteResponse::RowId(conn.last_insert_rowid()))
    })();
    finish_transaction(conn, result)
}

fn record_fleet_push(
    conn: &Connection,
    key: &str,
    value_json: &str,
    peers: &[String],
    author: &str,
) -> Result<WriteResponse> {
    validate_bounded_single_line(key, "fleet setting key", MAX_NAME_BYTES)?;
    if !key
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_'))
    {
        bail!("invalid fleet setting key");
    }
    validate_bounded_single_line(author, "fleet push author", MAX_IDENTITY_BYTES)?;
    if peers.is_empty() || peers.len() > MAX_FLEET_PEERS {
        bail!("fleet peer count is outside the finite writer envelope");
    }
    let mut prior: Option<&str> = None;
    for peer in peers {
        validate_identity(peer, "fleet peer id")?;
        if prior.is_some_and(|prior| prior >= peer.as_str()) {
            bail!("fleet peers must be strictly sorted and unique");
        }
        prior = Some(peer);
    }
    if value_json.len() > MAX_REVISION_SPEC_BYTES {
        bail!("fleet setting value exceeds the finite writer envelope");
    }
    mackes_mesh_types::workloads::reject_duplicate_json_keys(value_json)
        .context("fleet setting value contains duplicate JSON keys")?;
    let value: serde_json::Value =
        serde_json::from_str(value_json).context("fleet setting value is not JSON")?;
    validate_json_shape(&value, 0)?;
    let payload = serde_json::to_string(&serde_json::json!({
        "key": key,
        "value": value_json,
        "peers": peers,
    }))
    .context("serializing fleet push payload")?;
    if payload.len() > MAX_REVISION_SPEC_BYTES {
        bail!("fleet push payload exceeds the finite writer envelope");
    }
    let message = format!("fleet push: {key}");

    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT revision_id FROM desired_config WHERE author = ?1 AND message = ?2 AND spec_json = ?3 AND state = 'approved' ORDER BY revision_id DESC LIMIT 1",
                (author, &message, &payload),
                |row| row.get(0),
            )
            .optional()?;
        if let Some(revision_id) = existing {
            let revision_id_text = revision_id.to_string();
            let mut statement = conn.prepare(
                "SELECT peer_id FROM fleet_settings_apply_log WHERE revision_id = ?1 AND key = ?2 ORDER BY peer_id",
            )?;
            let durable_peers = statement
                .query_map((&revision_id_text, key), |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if durable_peers == peers {
                return Ok(WriteResponse::RowId(revision_id));
            }
            bail!("fleet push replay found an incomplete durable transaction");
        }

        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO desired_config (author, message, spec_json, state, created_at) VALUES (?1, ?2, ?3, 'approved', ?4)",
            (author, &message, &payload, &now),
        )?;
        let revision_id = conn.last_insert_rowid();
        for peer in peers {
            conn.execute(
                "INSERT INTO fleet_settings_apply_log (peer_id, revision_id, key, applied_at, ok) VALUES (?1, ?2, ?3, ?4, 0)",
                (peer, revision_id.to_string(), key, &now),
            )?;
        }
        Ok(WriteResponse::RowId(revision_id))
    })();
    finish_transaction(conn, result)
}

fn active_ca(conn: &Connection, mesh_id: &str) -> Result<Option<(i64, String)>> {
    conn.query_row(
        "SELECT epoch, ca_cert_pem FROM nebula_ca WHERE mesh_id = ?1 AND retired_at IS NULL ORDER BY epoch DESC LIMIT 1",
        [mesh_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(Into::into)
}

fn mint_ca(conn: &Connection, mesh_id: &str, ca_cert_pem: &str) -> Result<WriteResponse> {
    validate_identity(mesh_id, "mesh id")?;
    validate_bounded_text(ca_cert_pem, "CA certificate", MAX_CERT_PEM_BYTES)?;
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        if let Some((epoch, existing)) = active_ca(conn, mesh_id)? {
            if epoch == 0 && existing == ca_cert_pem {
                return Ok(WriteResponse::Count(0));
            }
            bail!("active CA changed while minting mesh {mesh_id}");
        }
        Ok(WriteResponse::Count(conn.execute(
            "INSERT INTO nebula_ca (mesh_id, epoch, ca_cert_pem, retired_at) VALUES (?1, 0, ?2, NULL)",
            (mesh_id, ca_cert_pem),
        )?))
    })();
    finish_transaction(conn, result)
}

fn seed_lighthouse_ca(
    conn: &Connection,
    mesh_id: &str,
    epoch: i64,
    ca_cert_pem: &str,
) -> Result<WriteResponse> {
    validate_identity(mesh_id, "mesh id")?;
    if epoch < 0 {
        bail!("invalid CA epoch");
    }
    validate_bounded_text(ca_cert_pem, "CA certificate", MAX_CERT_PEM_BYTES)?;
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        if let Some((active_epoch, active_cert)) = active_ca(conn, mesh_id)? {
            if active_epoch == epoch && active_cert == ca_cert_pem {
                return Ok(WriteResponse::Count(0));
            }
            bail!("active CA conflicts with lighthouse enrollment for mesh {mesh_id}");
        }
        let existing: Option<String> = conn
            .query_row(
                "SELECT ca_cert_pem FROM nebula_ca WHERE mesh_id = ?1 AND epoch = ?2",
                (mesh_id, epoch),
                |row| row.get(0),
            )
            .optional()?;
        match existing {
            Some(existing) if existing == ca_cert_pem => Ok(WriteResponse::Count(conn.execute(
                "UPDATE nebula_ca SET retired_at = NULL WHERE mesh_id = ?1 AND epoch = ?2",
                (mesh_id, epoch),
            )?)),
            Some(_) => bail!("durable CA epoch conflicts with lighthouse enrollment"),
            None => Ok(WriteResponse::Count(conn.execute(
                "INSERT INTO nebula_ca (mesh_id, epoch, ca_cert_pem, retired_at) VALUES (?1, ?2, ?3, NULL)",
                (mesh_id, epoch, ca_cert_pem),
            )?)),
        }
    })();
    finish_transaction(conn, result)
}

fn upsert_peer_cert(
    conn: &Connection,
    mesh_id: &str,
    expected_epoch: i64,
    peer: &CaPeerCertWrite,
) -> Result<WriteResponse> {
    validate_identity(mesh_id, "mesh id")?;
    validate_peer(peer, Some(expected_epoch))?;
    if peer.created_at.is_some() {
        bail!("peer signing cannot override the durable creation timestamp");
    }
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        let active = active_ca(conn, mesh_id)?.map(|(epoch, _)| epoch);
        if active != Some(expected_epoch) {
            bail!(
                "active CA epoch changed while signing for mesh {mesh_id}: expected {expected_epoch}, found {active:?}"
            );
        }
        let existing: Option<(String, String, i64, Option<String>, Option<i64>)> = conn
            .query_row(
                "SELECT cert_pem, overlay_ip, expires_at, public_key_pem, revoked_at \
                 FROM nebula_peer_certs WHERE node_id = ?1 AND epoch = ?2",
                rusqlite::params![peer.node_id, peer.epoch],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        if existing.as_ref().is_some_and(
            |(cert_pem, overlay_ip, expires_at, public_key_pem, revoked_at)| {
                revoked_at.is_none()
                    && cert_pem == &peer.cert_pem
                    && overlay_ip == &peer.overlay_ip
                    && expires_at == &peer.expires_at
                    && peer
                        .public_key_pem
                        .as_ref()
                        .is_none_or(|incoming| public_key_pem.as_ref() == Some(incoming))
            },
        ) {
            return Ok(WriteResponse::Count(0));
        }
        if existing
            .as_ref()
            .is_some_and(|(_, _, _, _, revoked_at)| revoked_at.is_some())
        {
            bail!(
                "durable peer certificate {} epoch {} is revoked",
                peer.node_id,
                peer.epoch
            );
        }
        let count = conn.execute(
            "INSERT INTO nebula_peer_certs (node_id, epoch, cert_pem, overlay_ip, expires_at, public_key_pem) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(node_id, epoch) DO UPDATE SET cert_pem = excluded.cert_pem, \
             overlay_ip = excluded.overlay_ip, expires_at = excluded.expires_at, \
             public_key_pem = COALESCE(excluded.public_key_pem, nebula_peer_certs.public_key_pem), revoked_at = NULL",
            rusqlite::params![
                peer.node_id,
                peer.epoch,
                peer.cert_pem,
                peer.overlay_ip,
                peer.expires_at,
                peer.public_key_pem,
            ],
        )?;
        Ok(WriteResponse::Count(count))
    })();
    finish_transaction(conn, result)
}

fn revoke_peer_cert(conn: &Connection, node_id: &str, revoked_at: i64) -> Result<WriteResponse> {
    validate_identity(node_id, "peer node id")?;
    if revoked_at < 0 {
        bail!("invalid peer revocation timestamp");
    }

    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        let active: i64 = conn.query_row(
            "SELECT COUNT(*) FROM nebula_peer_certs WHERE node_id = ?1 AND revoked_at IS NULL",
            [node_id],
            |row| row.get(0),
        )?;
        if active == 0 {
            return Ok(WriteResponse::Count(0));
        }
        let count = conn.execute(
            "UPDATE nebula_peer_certs SET revoked_at = ?1 WHERE node_id = ?2 AND revoked_at IS NULL",
            (revoked_at, node_id),
        )?;
        if i64::try_from(count)? != active {
            bail!("peer revocation changed under the writer transaction");
        }
        Ok(WriteResponse::Count(count))
    })();
    finish_transaction(conn, result)
}

fn restore_ca_backup(
    conn: &Connection,
    mesh_id: &str,
    ca_certs: &[CaCertWrite],
    peer_certs: &[CaPeerCertWrite],
) -> Result<WriteResponse> {
    validate_ca_bundle(mesh_id, ca_certs, peer_certs)?;
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        if let Some(incoming_active) = ca_certs
            .iter()
            .find(|certificate| certificate.retired_at.is_none())
        {
            let conflicting: i64 = conn.query_row(
                "SELECT COUNT(*) FROM nebula_ca WHERE mesh_id = ?1 AND retired_at IS NULL AND epoch != ?2",
                rusqlite::params![mesh_id, incoming_active.epoch],
                |row| row.get(0),
            )?;
            if conflicting > 0 {
                bail!("CA restore would create multiple active issuers");
            }
        }
        let mut missing_ca = Vec::new();
        for ca in ca_certs {
            let existing: Option<(String, i64, Option<i64>)> = conn
                .query_row(
                    "SELECT ca_cert_pem, created_at, retired_at FROM nebula_ca WHERE mesh_id = ?1 AND epoch = ?2",
                    rusqlite::params![mesh_id, ca.epoch],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            match existing {
                Some((cert, created_at, retired_at))
                    if cert == ca.ca_cert_pem
                        && created_at == ca.created_at
                        && retired_at == ca.retired_at => {}
                Some(_) => bail!(
                    "CA restore conflicts with durable issuer {mesh_id} epoch {}",
                    ca.epoch
                ),
                None => missing_ca.push(ca),
            }
        }
        let mut missing_peers = Vec::new();
        for peer in peer_certs {
            let existing: Option<(String, String, Option<String>, Option<i64>, i64, Option<i64>)> =
                conn.query_row(
                    "SELECT cert_pem, overlay_ip, public_key_pem, created_at, expires_at, revoked_at FROM nebula_peer_certs WHERE node_id = ?1 AND epoch = ?2",
                    rusqlite::params![peer.node_id, peer.epoch],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
                .optional()?;
            match existing {
                Some((cert, overlay_ip, public_key, created_at, expires_at, revoked_at))
                    if cert == peer.cert_pem
                        && overlay_ip == peer.overlay_ip
                        && public_key == peer.public_key_pem
                        && created_at == peer.created_at
                        && expires_at == peer.expires_at
                        && revoked_at.is_none() => {}
                Some(_) => bail!(
                    "CA restore conflicts with durable peer {} epoch {}",
                    peer.node_id,
                    peer.epoch
                ),
                None => missing_peers.push(peer),
            }
        }

        let mut count = 0;
        for ca in missing_ca {
            count += conn.execute(
                "INSERT INTO nebula_ca (mesh_id, epoch, ca_cert_pem, created_at, retired_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![mesh_id, ca.epoch, ca.ca_cert_pem, ca.created_at, ca.retired_at],
            )?;
        }
        for peer in missing_peers {
            count += conn.execute(
                "INSERT INTO nebula_peer_certs (node_id, epoch, cert_pem, overlay_ip, public_key_pem, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![peer.node_id, peer.epoch, peer.cert_pem, peer.overlay_ip, peer.public_key_pem, peer.created_at, peer.expires_at],
            )?;
        }
        Ok(WriteResponse::Count(count))
    })();
    finish_transaction(conn, result)
}

fn rotate_ca(
    conn: &Connection,
    mesh_id: &str,
    expected_active_epoch: Option<i64>,
    new_epoch: i64,
    ca_cert_pem: &str,
    peer_certs: &[CaPeerCertWrite],
) -> Result<WriteResponse> {
    validate_identity(mesh_id, "mesh id")?;
    validate_bounded_text(ca_cert_pem, "CA certificate", MAX_CERT_PEM_BYTES)?;
    validate_ca_payload_bytes(std::iter::once(ca_cert_pem), peer_certs)?;
    if new_epoch < 0 || expected_active_epoch.is_some_and(|epoch| epoch >= new_epoch) {
        bail!("invalid CA rotation epoch transition");
    }
    validate_rotation_peers(peer_certs, new_epoch)?;
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| {
        let active = active_ca(conn, mesh_id)?;
        if active.as_ref().map(|(epoch, _)| *epoch) == Some(new_epoch) {
            if active.as_ref().is_some_and(|(_, pem)| pem == ca_cert_pem)
                && rotation_rows_match(conn, new_epoch, peer_certs)?
            {
                return Ok(WriteResponse::Count(0));
            }
            bail!("CA rotation retry does not match durable epoch {new_epoch}");
        }
        let actual = active.as_ref().map(|(epoch, _)| *epoch);
        if actual != expected_active_epoch {
            bail!(
                "active CA changed during rotation: expected {expected_active_epoch:?}, found {actual:?}"
            );
        }
        if actual.is_some() {
            conn.execute(
                "UPDATE nebula_ca SET retired_at = unixepoch() WHERE mesh_id = ?1 AND retired_at IS NULL",
                [mesh_id],
            )?;
        }
        let mut count = conn.execute(
            "INSERT INTO nebula_ca (mesh_id, epoch, ca_cert_pem, retired_at) VALUES (?1, ?2, ?3, NULL)",
            rusqlite::params![mesh_id, new_epoch, ca_cert_pem],
        )?;
        for peer in peer_certs {
            count += conn.execute(
                "INSERT INTO nebula_peer_certs (node_id, epoch, cert_pem, overlay_ip, expires_at, public_key_pem) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![peer.node_id, peer.epoch, peer.cert_pem, peer.overlay_ip, peer.expires_at, peer.public_key_pem],
            )?;
        }
        Ok(WriteResponse::Count(count))
    })();
    finish_transaction(conn, result)
}

fn rotation_rows_match(conn: &Connection, epoch: i64, peers: &[CaPeerCertWrite]) -> Result<bool> {
    let durable_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM nebula_peer_certs WHERE epoch = ?1",
        [epoch],
        |row| row.get(0),
    )?;
    if usize::try_from(durable_count).ok() != Some(peers.len()) {
        return Ok(false);
    }
    for peer in peers {
        let row: Option<(String, String, Option<String>, i64)> = conn
            .query_row(
                "SELECT cert_pem, overlay_ip, public_key_pem, expires_at FROM nebula_peer_certs WHERE node_id = ?1 AND epoch = ?2",
                rusqlite::params![peer.node_id, peer.epoch],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if row
            != Some((
                peer.cert_pem.clone(),
                peer.overlay_ip.clone(),
                peer.public_key_pem.clone(),
                peer.expires_at,
            ))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_ca_bundle(
    mesh_id: &str,
    ca_certs: &[CaCertWrite],
    peer_certs: &[CaPeerCertWrite],
) -> Result<()> {
    validate_identity(mesh_id, "mesh id")?;
    if ca_certs.is_empty() || ca_certs.len() > 1024 || peer_certs.len() > 4096 {
        bail!("CA restore row count is outside the finite writer envelope");
    }
    validate_ca_payload_bytes(
        ca_certs
            .iter()
            .map(|certificate| certificate.ca_cert_pem.as_str()),
        peer_certs,
    )?;
    let mut epochs = std::collections::HashSet::new();
    let mut active = 0_usize;
    for ca in ca_certs {
        validate_bounded_text(&ca.ca_cert_pem, "CA certificate", MAX_CERT_PEM_BYTES)?;
        if ca.epoch < 0
            || ca.created_at < 0
            || ca.retired_at.is_some_and(|retired_at| retired_at < 0)
            || !epochs.insert(ca.epoch)
        {
            bail!("CA restore contains an invalid or duplicate issuer epoch");
        }
        active += usize::from(ca.retired_at.is_none());
    }
    if active > 1 {
        bail!("CA restore contains more than one active issuer");
    }
    let mut peer_keys = std::collections::HashSet::new();
    let mut overlay_keys = std::collections::HashSet::new();
    for peer in peer_certs {
        validate_peer(peer, None)?;
        if !epochs.contains(&peer.epoch)
            || !peer_keys.insert((peer.node_id.clone(), peer.epoch))
            || !overlay_keys.insert((peer.overlay_ip.clone(), peer.epoch))
        {
            bail!("CA restore contains a duplicate peer or missing issuer");
        }
    }
    Ok(())
}

fn validate_rotation_peers(peers: &[CaPeerCertWrite], epoch: i64) -> Result<()> {
    if peers.len() > 4096 {
        bail!("CA rotation peer count exceeds the finite writer envelope");
    }
    let mut nodes = std::collections::HashSet::new();
    let mut overlays = std::collections::HashSet::new();
    for peer in peers {
        validate_peer(peer, Some(epoch))?;
        if !nodes.insert(&peer.node_id) || !overlays.insert(&peer.overlay_ip) {
            bail!("CA rotation contains duplicate peer identity or overlay IP");
        }
    }
    Ok(())
}

fn validate_ca_payload_bytes<'a>(
    ca_certificates: impl IntoIterator<Item = &'a str>,
    peers: &[CaPeerCertWrite],
) -> Result<()> {
    let mut total = 0_usize;
    for certificate in ca_certificates {
        total = total
            .checked_add(certificate.len())
            .context("CA writer payload length overflow")?;
    }
    for peer in peers {
        total = total
            .checked_add(peer.node_id.len())
            .and_then(|value| value.checked_add(peer.cert_pem.len()))
            .and_then(|value| value.checked_add(peer.overlay_ip.len()))
            .and_then(|value| {
                value.checked_add(peer.public_key_pem.as_ref().map_or(0, String::len))
            })
            .context("CA writer payload length overflow")?;
    }
    if total > MAX_CA_PAYLOAD_BYTES {
        bail!("CA writer payload exceeds the finite byte envelope");
    }
    Ok(())
}

fn validate_peer(peer: &CaPeerCertWrite, expected_epoch: Option<i64>) -> Result<()> {
    validate_identity(&peer.node_id, "peer node id")?;
    validate_bounded_text(&peer.cert_pem, "peer certificate", MAX_CERT_PEM_BYTES)?;
    if let Some(public_key) = peer.public_key_pem.as_deref() {
        validate_bounded_text(public_key, "peer public key", MAX_PUBLIC_KEY_BYTES)?;
    }
    if peer.epoch < 0
        || expected_epoch.is_some_and(|epoch| peer.epoch != epoch)
        || peer.overlay_ip.parse::<std::net::Ipv4Addr>().is_err()
        || peer.created_at.is_some_and(|created_at| created_at < 0)
        || peer.expires_at < 0
    {
        bail!("invalid CA peer certificate row");
    }
    Ok(())
}

fn validate_identity(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > MAX_IDENTITY_BYTES
        || value.trim() != value
        || matches!(value, "." | "..")
        || value
            .chars()
            .any(|character| matches!(character, '/' | '\\') || character.is_control())
    {
        bail!("invalid {label}");
    }
    Ok(())
}

fn validate_nonempty(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("empty {label}");
    }
    Ok(())
}

fn validate_bounded_text(value: &str, label: &str, maximum: usize) -> Result<()> {
    validate_nonempty(value, label)?;
    if value.len() > maximum || value.contains('\0') {
        bail!("invalid or oversized {label}");
    }
    Ok(())
}

fn validate_bounded_single_line(value: &str, label: &str, maximum: usize) -> Result<()> {
    validate_bounded_text(value, label, maximum)?;
    if value.chars().any(char::is_control) {
        bail!("invalid {label}");
    }
    Ok(())
}

fn validate_json_shape(value: &serde_json::Value, depth: usize) -> Result<()> {
    if depth > MAX_JSON_DEPTH {
        bail!("JSON payload exceeds the maximum nesting depth");
    }
    match value {
        serde_json::Value::Array(items) => {
            if items.len() > MAX_JSON_CONTAINER_ITEMS {
                bail!("JSON array exceeds the finite item envelope");
            }
            for item in items {
                validate_json_shape(item, depth + 1)?;
            }
        }
        serde_json::Value::Object(fields) => {
            if fields.len() > MAX_JSON_CONTAINER_ITEMS {
                bail!("JSON object exceeds the finite field envelope");
            }
            for (key, item) in fields {
                validate_bounded_text(key, "JSON object key", MAX_NAME_BYTES)?;
                validate_json_shape(item, depth + 1)?;
            }
        }
        serde_json::Value::String(text) => {
            if text.len() > MAX_EVENT_DETAIL_BYTES || text.contains('\0') {
                bail!("JSON string exceeds the finite writer envelope");
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
    Ok(())
}

fn finish_transaction(conn: &Connection, result: Result<WriteResponse>) -> Result<WriteResponse> {
    match result {
        Ok(response) => {
            conn.execute_batch("COMMIT")?;
            Ok(response)
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn write_frame(stream: &mut UnixStream, payload: &[u8]) -> Result<()> {
    if payload.len() > MAX_FRAME_BYTES {
        bail!("SQLite writer frame exceeds {MAX_FRAME_BYTES} bytes");
    }
    let length = u32::try_from(payload.len()).context("SQLite writer frame length overflow")?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(payload)?;
    Ok(())
}

fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header)?;
    let length = usize::try_from(u32::from_be_bytes(header))?;
    if length > MAX_FRAME_BYTES {
        bail!("SQLite writer frame exceeds {MAX_FRAME_BYTES} bytes");
    }
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reconcile_event(event_id: u64, node_id: &str) -> crate::events::Event {
        crate::events::Event {
            event_id,
            kind: crate::events::EventKind::Reconcile,
            node_id: node_id.to_owned(),
            timestamp_ms: 1,
            detail: serde_json::json!({"event": event_id}),
        }
    }

    fn stop(server: WriterServer, shutdown: &AtomicBool) {
        shutdown.store(true, Ordering::Relaxed);
        server.join().expect("writer joins");
    }

    #[test]
    fn typed_writer_serializes_mutations_and_survives_restart() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = temp.path().join("mackesd.db");
        let socket = temp.path().join("writer.sock");
        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("start writer");
        let response = request(
            &socket,
            WriteOp::UpsertNode {
                node_id: "peer:a".into(),
                name: "a".into(),
                public_key: "pk".into(),
                region: None,
            },
        )
        .expect("upsert");
        assert_eq!(response.into_count().expect("count"), 1);
        stop(server, &shutdown);

        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("restart writer");
        let response = request(
            &socket,
            WriteOp::SetNodeHealth {
                node_id: "peer:a".into(),
                health: "healthy".into(),
            },
        )
        .expect("health");
        assert!(response.into_changed().expect("changed"));
        stop(server, &shutdown);
    }

    #[test]
    fn reconcile_batch_is_transactional_bounded_and_restart_replay_safe() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = temp.path().join("mackesd.db");
        let socket = temp.path().join("writer.sock");
        drop(crate::store::open(&db).expect("initialize store"));

        let fault = Connection::open(&db).expect("open fault fixture");
        fault
            .execute_batch(
                "CREATE TRIGGER reject_second_reconcile BEFORE INSERT ON events \
                 WHEN NEW.payload_json LIKE '%\"event_id\":2%' \
                 BEGIN SELECT RAISE(ABORT, 'fault injection'); END;",
            )
            .expect("install fault trigger");
        drop(fault);

        let events = vec![
            reconcile_event(1, "peer:worker"),
            reconcile_event(2, "peer:worker"),
        ];
        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("start writer");
        assert!(request(
            &socket,
            WriteOp::AppendReconcileEvents {
                events: events.clone(),
            },
        )
        .expect("fault response")
        .into_count()
        .is_err());
        stop(server, &shutdown);

        let fixture = Connection::open(&db).expect("open rollback fixture");
        let count: i64 = fixture
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .expect("count rolled-back events");
        assert_eq!(count, 0, "the first row must roll back with the second");
        fixture
            .execute_batch("DROP TRIGGER reject_second_reconcile")
            .expect("remove fault trigger");
        drop(fixture);

        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("restart writer");
        assert_eq!(
            request(
                &socket,
                WriteOp::AppendReconcileEvents {
                    events: events.clone(),
                },
            )
            .expect("append batch")
            .into_count()
            .expect("inserted count"),
            2
        );
        assert_eq!(
            request(&socket, WriteOp::AppendReconcileEvents { events })
                .expect("replay batch")
                .into_count()
                .expect("replay count"),
            0
        );
        assert!(request(
            &socket,
            WriteOp::AppendReconcileEvents {
                events: vec![reconcile_event(3, "peer:worker"); MAX_RECONCILE_EVENTS + 1],
            },
        )
        .expect("oversized response")
        .into_count()
        .is_err());
        stop(server, &shutdown);

        let reader = Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open reader");
        let count: i64 = reader
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .expect("count durable events");
        assert_eq!(count, 2, "replay and rejection must not append rows");
        let audit_rows = crate::store::load_audit_rows(&reader).expect("load audit rows");
        assert!(matches!(
            crate::audit::verify(&audit_rows),
            crate::audit::VerifyOutcome::Intact { verified: 2, .. }
        ));
    }

    #[test]
    fn revision_and_event_operations_are_bounded_transactional_and_idempotent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = temp.path().join("mackesd.db");
        let socket = temp.path().join("writer.sock");
        let seed = Connection::open(&db).expect("seed open");
        super::super::migrate(&seed).expect("migrate");
        seed.execute(
            "INSERT INTO desired_config (author, message, spec_json, state, created_at) VALUES (?1, ?2, ?3, 'verified', ?4)",
            ("operator", "source", r#"{"enabled":true}"#, "2026-08-08T12:00:00Z"),
        )
        .expect("seed desired revision");
        seed.execute(
            "INSERT INTO desired_config (author, message, spec_json, state, created_at) VALUES (?1, ?2, ?3, 'verified', ?4)",
            (
                "operator",
                "hostile source",
                r#"{"enabled":true,"enabled":false}"#,
                "2026-08-08T12:00:01Z",
            ),
        )
        .expect("seed hostile desired revision");
        drop(seed);

        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("start writer");
        let revision = WriteOp::CreateApprovedRevision {
            target_revision_id: 1,
            author: "operator".into(),
            message: "Rollback to 1 (peers=false)".into(),
            created_at: "2026-08-08T12:01:00Z".into(),
        };
        let first_revision = request(&socket, revision.clone())
            .expect("create revision")
            .into_row_id()
            .expect("revision row id");
        let replayed_revision = request(&socket, revision)
            .expect("replay revision")
            .into_row_id()
            .expect("replayed revision row id");
        assert_eq!(first_revision, replayed_revision);

        let event = WriteOp::AppendEventRecord {
            event_id: 1_786_186_860_000,
            kind: "lifecycle".into(),
            node_id: "peer:a".into(),
            timestamp_ms: 1_786_186_860_000,
            detail: serde_json::json!({"action": "writer_boundary"}),
        };
        let first_event = request(&socket, event.clone())
            .expect("append event")
            .into_row_id()
            .expect("event row id");
        let replayed_event = request(&socket, event)
            .expect("replay event")
            .into_row_id()
            .expect("replayed event row id");
        assert_eq!(first_event, replayed_event);

        let hostile = request(
            &socket,
            WriteOp::AppendEventRecord {
                event_id: 1,
                kind: "lifecycle".into(),
                node_id: "peer:a".into(),
                timestamp_ms: 1,
                detail: serde_json::json!(["not", "an", "object"]),
            },
        )
        .expect("hostile event response");
        assert!(matches!(hostile, WriteResponse::Error(error) if error.contains("JSON object")));

        let missing = request(
            &socket,
            WriteOp::CreateApprovedRevision {
                target_revision_id: 999,
                author: "operator".into(),
                message: "missing".into(),
                created_at: "2026-08-08T12:02:00Z".into(),
            },
        )
        .expect("missing source response");
        assert!(
            matches!(missing, WriteResponse::Error(error) if error.contains("loading desired revision"))
        );
        let duplicate_payload = request(
            &socket,
            WriteOp::CreateApprovedRevision {
                target_revision_id: 2,
                author: "operator".into(),
                message: "hostile duplicate payload".into(),
                created_at: "2026-08-08T12:03:00Z".into(),
            },
        )
        .expect("duplicate payload response");
        assert!(matches!(
            duplicate_payload,
            WriteResponse::Error(error) if error.contains("duplicate JSON keys")
        ));
        stop(server, &shutdown);

        let reader = Connection::open(&db).expect("reader open");
        let revisions: i64 = reader
            .query_row("SELECT COUNT(*) FROM desired_config", [], |row| row.get(0))
            .expect("revision count");
        let events: i64 = reader
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .expect("event count");
        assert_eq!(
            revisions, 3,
            "replay and rejected source payloads must not add rows"
        );
        assert_eq!(events, 1, "replay and hostile event must not add rows");
        let audit_rows = super::super::load_audit_rows(&reader).expect("audit rows");
        assert!(matches!(
            crate::audit::verify(&audit_rows),
            crate::audit::VerifyOutcome::Intact { verified: 1, .. }
        ));
    }

    #[test]
    fn fleet_and_lighthouse_operations_are_transactional_bounded_and_restart_idempotent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = temp.path().join("mackesd.db");
        let socket = temp.path().join("writer.sock");
        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("start writer");

        let fleet = WriteOp::RecordFleetPush {
            key: "theme.mode".into(),
            value_json: r#""dark""#.into(),
            peers: vec!["peer:a".into(), "peer:b".into()],
            author: "peer:operator".into(),
        };
        let fault = Connection::open(&db).expect("open fault fixture");
        fault
            .execute_batch(
                "CREATE TRIGGER reject_second_fleet_log BEFORE INSERT ON fleet_settings_apply_log \
                 WHEN NEW.peer_id = 'peer:b' BEGIN SELECT RAISE(ABORT, 'injected fleet log failure'); END;",
            )
            .expect("install fleet fault");
        let rejected = request(&socket, fleet.clone()).expect("typed failure response");
        assert!(matches!(rejected, WriteResponse::Error(_)));
        fault
            .execute_batch("DROP TRIGGER reject_second_fleet_log")
            .expect("remove fleet fault");
        let empty_prefix: i64 = fault
            .query_row(
                "SELECT COUNT(*) FROM desired_config WHERE message = 'fleet push: theme.mode'",
                [],
                |row| row.get(0),
            )
            .expect("count rejected prefix");
        assert_eq!(
            empty_prefix, 0,
            "failed log batch must roll back its revision"
        );

        let first_revision = request(&socket, fleet.clone())
            .expect("record fleet push")
            .into_row_id()
            .expect("fleet revision id");
        request(
            &socket,
            WriteOp::SeedLighthouseCa {
                mesh_id: "mesh-enrolled".into(),
                epoch: 7,
                ca_cert_pem: "ca:7".into(),
            },
        )
        .expect("seed lighthouse CA")
        .into_count()
        .expect("CA seed count");
        stop(server, &shutdown);

        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("restart writer");
        let replayed_revision = request(&socket, fleet)
            .expect("replay fleet push")
            .into_row_id()
            .expect("replayed fleet revision id");
        assert_eq!(replayed_revision, first_revision);
        let replayed_ca = request(
            &socket,
            WriteOp::SeedLighthouseCa {
                mesh_id: "mesh-enrolled".into(),
                epoch: 7,
                ca_cert_pem: "ca:7".into(),
            },
        )
        .expect("replay lighthouse CA")
        .into_count()
        .expect("replayed CA count");
        assert_eq!(replayed_ca, 0);

        let duplicate_value = request(
            &socket,
            WriteOp::RecordFleetPush {
                key: "theme.hostile".into(),
                value_json: r#"{"enabled":true,"enabled":false}"#.into(),
                peers: vec!["peer:a".into()],
                author: "peer:operator".into(),
            },
        )
        .expect("hostile fleet response");
        assert!(matches!(
            duplicate_value,
            WriteResponse::Error(error) if error.contains("duplicate JSON keys")
        ));
        let conflicting_ca = request(
            &socket,
            WriteOp::SeedLighthouseCa {
                mesh_id: "mesh-enrolled".into(),
                epoch: 8,
                ca_cert_pem: "ca:8".into(),
            },
        )
        .expect("conflicting CA response");
        assert!(matches!(
            conflicting_ca,
            WriteResponse::Error(error) if error.contains("conflicts")
        ));

        let reader = Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("reader");
        let revisions: i64 = reader
            .query_row(
                "SELECT COUNT(*) FROM desired_config WHERE message = 'fleet push: theme.mode'",
                [],
                |row| row.get(0),
            )
            .expect("fleet revision count");
        let logs: i64 = reader
            .query_row("SELECT COUNT(*) FROM fleet_settings_apply_log", [], |row| {
                row.get(0)
            })
            .expect("fleet log count");
        let ca_rows: i64 = reader
            .query_row("SELECT COUNT(*) FROM nebula_ca", [], |row| row.get(0))
            .expect("CA row count");
        assert_eq!(revisions, 1);
        assert_eq!(logs, 2);
        assert_eq!(ca_rows, 1);
        stop(server, &shutdown);
    }

    #[test]
    fn hostile_schema_and_oversized_frames_fail_closed_without_killing_owner() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = temp.path().join("mackesd.db");
        let socket = temp.path().join("writer.sock");
        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("start writer");

        let mut stream = UnixStream::connect(&socket).expect("connect");
        let bad = br#"{"schema_version":99,"operation":{"op":"set_node_role","node_id":"peer:a","role":"peer"}}"#;
        write_frame(&mut stream, bad).expect("write hostile request");
        let response: WriteResponse =
            serde_json::from_slice(&read_frame(&mut stream).expect("response")).expect("decode");
        assert!(matches!(response, WriteResponse::Error(error) if error.contains("unsupported")));

        let mut stream = UnixStream::connect(&socket).expect("connect duplicate-key request");
        let duplicate = br#"{"schema_version":1,"schema_version":1,"operation":{"op":"set_node_role","node_id":"peer:a","role":"peer"}}"#;
        write_frame(&mut stream, duplicate).expect("write duplicate-key request");
        let response: WriteResponse =
            serde_json::from_slice(&read_frame(&mut stream).expect("response")).expect("decode");
        assert!(
            matches!(response, WriteResponse::Error(error) if error.contains("duplicate JSON keys"))
        );

        let mut stream = UnixStream::connect(&socket).expect("connect unknown-field request");
        let unknown = br#"{"schema_version":1,"operation":{"op":"set_node_role","node_id":"peer:a","role":"peer","sql":"DELETE FROM nodes"}}"#;
        write_frame(&mut stream, unknown).expect("write unknown-field request");
        let response: WriteResponse =
            serde_json::from_slice(&read_frame(&mut stream).expect("response")).expect("decode");
        assert!(matches!(response, WriteResponse::Error(error) if error.contains("unknown field")));

        let mut stream = UnixStream::connect(&socket).expect("connect oversized");
        stream
            .write_all(&u32::try_from(MAX_FRAME_BYTES + 1).unwrap().to_be_bytes())
            .expect("header");
        let response: WriteResponse =
            serde_json::from_slice(&read_frame(&mut stream).expect("response")).expect("decode");
        assert!(matches!(response, WriteResponse::Error(error) if error.contains("exceeds")));

        request(
            &socket,
            WriteOp::MintCa {
                mesh_id: "mesh-bounds".into(),
                ca_cert_pem: "ca:0".into(),
            },
        )
        .expect("mint response")
        .into_count()
        .expect("mint accepted");
        let oversized = request(
            &socket,
            WriteOp::UpsertPeerCert {
                mesh_id: "mesh-bounds".into(),
                expected_epoch: 0,
                peer: CaPeerCertWrite {
                    cert_pem: "x".repeat(MAX_CERT_PEM_BYTES + 1),
                    ..peer("peer:oversized", 0, "10.42.0.7")
                },
            },
        )
        .expect("oversized field response");
        assert!(
            matches!(oversized, WriteResponse::Error(error) if error.contains("oversized peer certificate"))
        );

        let healthy = request(
            &socket,
            WriteOp::UpsertNode {
                node_id: "peer:b".into(),
                name: "b".into(),
                public_key: "pk".into(),
                region: None,
            },
        )
        .expect("owner remains available");
        assert_eq!(healthy.into_count().expect("count"), 1);
        stop(server, &shutdown);
    }

    #[test]
    fn ordinary_read_only_connection_rejects_untyped_sql_write() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = temp.path().join("mackesd.db");
        let owner = Connection::open(&db).expect("owner open");
        super::super::migrate(&owner).expect("migrate");
        drop(owner);
        let reader = Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("reader open");
        let error = reader
            .execute("DELETE FROM events", [])
            .expect_err("write denied");
        assert!(matches!(error, rusqlite::Error::SqliteFailure(_, _)));
    }

    #[test]
    fn missing_owner_fails_after_a_bounded_readiness_wait() {
        let temp = tempfile::tempdir().expect("tempdir");
        let started = Instant::now();
        let error = connect_bounded(&temp.path().join("missing.sock"), Duration::from_millis(75))
            .expect_err("missing owner must fail");
        assert!(error.to_string().contains("unavailable"));
        assert!(started.elapsed() >= Duration::from_millis(75));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    fn peer(node_id: &str, epoch: i64, overlay_ip: &str) -> CaPeerCertWrite {
        CaPeerCertWrite {
            node_id: node_id.into(),
            epoch,
            cert_pem: format!("cert:{node_id}:{epoch}"),
            overlay_ip: overlay_ip.into(),
            public_key_pem: Some(format!("public:{node_id}")),
            created_at: None,
            expires_at: 0,
        }
    }

    #[test]
    fn ca_compare_and_swap_rejects_stale_signing_and_survives_restart() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = temp.path().join("mackesd.db");
        let socket = temp.path().join("writer.sock");
        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("start writer");

        request(
            &socket,
            WriteOp::MintCa {
                mesh_id: "mesh-a".into(),
                ca_cert_pem: "ca:0".into(),
            },
        )
        .expect("mint request")
        .into_count()
        .expect("mint accepted");
        let stale = request(
            &socket,
            WriteOp::UpsertPeerCert {
                mesh_id: "mesh-a".into(),
                expected_epoch: 1,
                peer: peer("peer:stale", 1, "10.42.0.2"),
            },
        )
        .expect("stale response");
        assert!(
            matches!(stale, WriteResponse::Error(error) if error.contains("active CA epoch changed"))
        );
        request(
            &socket,
            WriteOp::UpsertPeerCert {
                mesh_id: "mesh-a".into(),
                expected_epoch: 0,
                peer: peer("peer:a", 0, "10.42.0.1"),
            },
        )
        .expect("sign response")
        .into_count()
        .expect("sign accepted");
        stop(server, &shutdown);

        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("restart writer");
        let replayed = request(
            &socket,
            WriteOp::UpsertPeerCert {
                mesh_id: "mesh-a".into(),
                expected_epoch: 0,
                peer: peer("peer:a", 0, "10.42.0.1"),
            },
        )
        .expect("replayed sign response")
        .into_count()
        .expect("replayed sign accepted");
        assert_eq!(replayed, 0, "identical signing replay must be a no-op");
        request(
            &socket,
            WriteOp::InsertEvent {
                kind: "admin_action".into(),
                actor: "peer:operator".into(),
                payload_json: r#"{"action":"rotate_ca","mesh_id":"mesh-a"}"#.into(),
            },
        )
        .expect("seed pre-rotation audit event")
        .into_row_id()
        .expect("audit row id");
        let generation = vec![
            peer("peer:a", 1, "10.42.0.1"),
            peer("peer:b", 1, "10.42.0.2"),
        ];
        let fault = Connection::open(&db).expect("open rotation fault fixture");
        fault
            .execute_batch(
                "CREATE TRIGGER reject_second_rotation_peer BEFORE INSERT ON nebula_peer_certs \
                 WHEN NEW.node_id = 'peer:b' AND NEW.epoch = 1 \
                 BEGIN SELECT RAISE(ABORT, 'injected rotation peer failure'); END;",
            )
            .expect("install rotation fault");
        let rejected = request(
            &socket,
            WriteOp::RotateCa {
                mesh_id: "mesh-a".into(),
                expected_active_epoch: Some(0),
                new_epoch: 1,
                ca_cert_pem: "ca:1".into(),
                peer_certs: generation.clone(),
            },
        )
        .expect("typed failed rotation response");
        assert!(matches!(rejected, WriteResponse::Error(_)));
        let active_after_failure: i64 = fault
            .query_row(
                "SELECT epoch FROM nebula_ca WHERE mesh_id = 'mesh-a' AND retired_at IS NULL",
                [],
                |row| row.get(0),
            )
            .expect("active epoch after failed rotation");
        let failed_generation_rows: i64 = fault
            .query_row(
                "SELECT COUNT(*) FROM nebula_peer_certs WHERE epoch = 1",
                [],
                |row| row.get(0),
            )
            .expect("failed generation row count");
        assert_eq!(active_after_failure, 0);
        assert_eq!(failed_generation_rows, 0);
        fault
            .execute_batch("DROP TRIGGER reject_second_rotation_peer")
            .expect("remove rotation fault");
        request(
            &socket,
            WriteOp::RotateCa {
                mesh_id: "mesh-a".into(),
                expected_active_epoch: Some(0),
                new_epoch: 1,
                ca_cert_pem: "ca:1".into(),
                peer_certs: generation.clone(),
            },
        )
        .expect("rotation response")
        .into_count()
        .expect("rotation accepted");
        stop(server, &shutdown);

        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("second restart");
        let retried = request(
            &socket,
            WriteOp::RotateCa {
                mesh_id: "mesh-a".into(),
                expected_active_epoch: Some(0),
                new_epoch: 1,
                ca_cert_pem: "ca:1".into(),
                peer_certs: generation,
            },
        )
        .expect("retry response")
        .into_count()
        .expect("identical retry accepted");
        assert_eq!(retried, 0, "durable generation is idempotent after restart");
        let conflict = request(
            &socket,
            WriteOp::RotateCa {
                mesh_id: "mesh-a".into(),
                expected_active_epoch: Some(0),
                new_epoch: 1,
                ca_cert_pem: "ca:conflict".into(),
                peer_certs: vec![
                    peer("peer:a", 1, "10.42.0.1"),
                    peer("peer:b", 1, "10.42.0.2"),
                ],
            },
        )
        .expect("typed conflicting rotation response");
        assert!(matches!(
            conflict,
            WriteResponse::Error(error) if error.contains("does not match durable epoch")
        ));
        let reader = Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("reader");
        let active: i64 = reader
            .query_row(
                "SELECT epoch FROM nebula_ca WHERE mesh_id = 'mesh-a' AND retired_at IS NULL",
                [],
                |row| row.get(0),
            )
            .expect("active epoch");
        let stale_rows: i64 = reader
            .query_row(
                "SELECT COUNT(*) FROM nebula_peer_certs WHERE node_id = 'peer:stale'",
                [],
                |row| row.get(0),
            )
            .expect("stale count");
        assert_eq!(active, 1);
        assert_eq!(stale_rows, 0, "stale signer must be a no-op");
        let audit_rows = super::super::load_audit_rows(&reader).expect("audit rows");
        assert!(matches!(
            crate::audit::verify(&audit_rows),
            crate::audit::VerifyOutcome::Intact { verified: 1, .. }
        ));
        stop(server, &shutdown);
    }

    #[test]
    fn peer_revocation_is_atomic_conflict_safe_and_restart_replay_safe() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = temp.path().join("mackesd.db");
        let socket = temp.path().join("writer.sock");
        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("start writer");

        request(
            &socket,
            WriteOp::MintCa {
                mesh_id: "mesh-revoke".into(),
                ca_cert_pem: "ca:0".into(),
            },
        )
        .expect("mint request")
        .into_count()
        .expect("mint accepted");
        request(
            &socket,
            WriteOp::UpsertPeerCert {
                mesh_id: "mesh-revoke".into(),
                expected_epoch: 0,
                peer: peer("peer:revoke", 0, "10.42.0.8"),
            },
        )
        .expect("initial peer request")
        .into_count()
        .expect("initial peer accepted");
        request(
            &socket,
            WriteOp::RotateCa {
                mesh_id: "mesh-revoke".into(),
                expected_active_epoch: Some(0),
                new_epoch: 1,
                ca_cert_pem: "ca:1".into(),
                peer_certs: vec![peer("peer:revoke", 1, "10.42.0.8")],
            },
        )
        .expect("rotation request")
        .into_count()
        .expect("rotation accepted");
        request(
            &socket,
            WriteOp::InsertEvent {
                kind: "admin_action".into(),
                actor: "peer:operator".into(),
                payload_json: r#"{"action":"revoke","node_id":"peer:revoke"}"#.into(),
            },
        )
        .expect("audit seed request")
        .into_row_id()
        .expect("audit seed accepted");

        let fault = Connection::open(&db).expect("open fault fixture");
        fault
            .execute_batch(
                "CREATE TRIGGER reject_second_peer_revoke BEFORE UPDATE OF revoked_at ON nebula_peer_certs \
                 WHEN OLD.node_id = 'peer:revoke' AND OLD.epoch = 1 \
                 BEGIN SELECT RAISE(ABORT, 'injected revocation failure'); END;",
            )
            .expect("install revocation fault");
        let rejected = request(
            &socket,
            WriteOp::RevokePeerCert {
                node_id: "peer:revoke".into(),
                revoked_at: 1_234,
            },
        )
        .expect("failed revocation response");
        assert!(matches!(rejected, WriteResponse::Error(_)));
        let active_after_failure: i64 = fault
            .query_row(
                "SELECT COUNT(*) FROM nebula_peer_certs WHERE node_id = 'peer:revoke' AND revoked_at IS NULL",
                [],
                |row| row.get(0),
            )
            .expect("active rows after failed revocation");
        assert_eq!(active_after_failure, 2, "failed revocation must roll back");
        fault
            .execute_batch("DROP TRIGGER reject_second_peer_revoke")
            .expect("remove revocation fault");

        let revoked = request(
            &socket,
            WriteOp::RevokePeerCert {
                node_id: "peer:revoke".into(),
                revoked_at: 1_234,
            },
        )
        .expect("revocation response")
        .into_count()
        .expect("revocation accepted");
        assert_eq!(revoked, 2);
        let resurrection = request(
            &socket,
            WriteOp::UpsertPeerCert {
                mesh_id: "mesh-revoke".into(),
                expected_epoch: 1,
                peer: peer("peer:revoke", 1, "10.42.0.8"),
            },
        )
        .expect("resurrection response");
        assert!(matches!(
            resurrection,
            WriteResponse::Error(error) if error.contains("is revoked")
        ));
        drop(fault);
        stop(server, &shutdown);

        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("restart writer");
        let replayed = request(
            &socket,
            WriteOp::RevokePeerCert {
                node_id: "peer:revoke".into(),
                revoked_at: 1_234,
            },
        )
        .expect("replayed revocation response")
        .into_count()
        .expect("replayed revocation accepted");
        assert_eq!(replayed, 0, "durable revocation replay must be a no-op");
        stop(server, &shutdown);

        let reader = Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read-only verifier");
        let durable: (i64, i64) = reader
            .query_row(
                "SELECT COUNT(*), MIN(revoked_at) FROM nebula_peer_certs WHERE node_id = 'peer:revoke' AND revoked_at = 1234",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("durable revocation rows");
        assert_eq!(durable, (2, 1_234));
        let audit_rows = super::super::load_audit_rows(&reader).expect("audit rows");
        assert_eq!(
            audit_rows.len(),
            1,
            "revocation must not rewrite audit history"
        );
        assert!(matches!(
            crate::audit::verify(&audit_rows),
            crate::audit::VerifyOutcome::Intact { verified: 1, .. }
        ));
    }

    #[test]
    fn hostile_ca_restore_rolls_back_prefix_before_owner_accepts_next_request() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = temp.path().join("mackesd.db");
        let socket = temp.path().join("writer.sock");
        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("start writer");
        let fault = Connection::open(&db).expect("open fault fixture");
        fault
            .execute_batch(
                "CREATE TRIGGER reject_peer_restore BEFORE INSERT ON nebula_peer_certs \
                 BEGIN SELECT RAISE(ABORT, 'injected peer restore failure'); END;",
            )
            .expect("install peer restore fault");
        let hostile = request(
            &socket,
            WriteOp::RestoreCaBackup {
                mesh_id: "mesh-a".into(),
                ca_certs: vec![CaCertWrite {
                    epoch: 0,
                    ca_cert_pem: "ca:0".into(),
                    created_at: 1,
                    retired_at: None,
                }],
                peer_certs: vec![CaPeerCertWrite {
                    created_at: Some(1),
                    ..peer("peer:a", 0, "10.42.0.1")
                }],
            },
        )
        .expect("hostile response");
        assert!(
            matches!(hostile, WriteResponse::Error(_)),
            "hostile partial restore must return a typed error"
        );
        fault
            .execute_batch("DROP TRIGGER reject_peer_restore")
            .expect("remove peer restore fault");

        request(
            &socket,
            WriteOp::MintCa {
                mesh_id: "mesh-a".into(),
                ca_cert_pem: "ca:healthy".into(),
            },
        )
        .expect("healthy response")
        .into_count()
        .expect("owner remains healthy");
        let reader = Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("reader");
        let rows: i64 = reader
            .query_row("SELECT COUNT(*) FROM nebula_ca", [], |row| row.get(0))
            .expect("count CA rows");
        assert_eq!(rows, 1, "hostile restore must not commit a prefix");
        stop(server, &shutdown);
    }

    #[test]
    fn ca_restore_is_conflict_safe_and_restart_idempotent_without_touching_audit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = temp.path().join("mackesd.db");
        let socket = temp.path().join("writer.sock");
        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("start writer");

        request(
            &socket,
            WriteOp::InsertEvent {
                kind: "admin_action".into(),
                actor: "operator".into(),
                payload_json: r#"{"action":"before_ca_restore"}"#.into(),
            },
        )
        .expect("seed audit event")
        .into_row_id()
        .expect("audit event row id");
        let restore = WriteOp::RestoreCaBackup {
            mesh_id: "mesh-restore".into(),
            ca_certs: vec![CaCertWrite {
                epoch: 4,
                ca_cert_pem: "ca:4".into(),
                created_at: 100,
                retired_at: None,
            }],
            peer_certs: vec![CaPeerCertWrite {
                node_id: "peer:restore".into(),
                epoch: 4,
                cert_pem: "cert:restore:4".into(),
                overlay_ip: "10.42.0.44".into(),
                public_key_pem: Some("public:restore".into()),
                created_at: Some(101),
                expires_at: 200,
            }],
        };
        assert_eq!(
            request(&socket, restore.clone())
                .expect("first restore")
                .into_count()
                .expect("first restore count"),
            2
        );
        stop(server, &shutdown);

        let shutdown = Arc::new(AtomicBool::new(false));
        let server = start(&db, &socket, Arc::clone(&shutdown)).expect("restart writer");
        assert_eq!(
            request(&socket, restore.clone())
                .expect("restore replay")
                .into_count()
                .expect("restore replay count"),
            0,
            "an identical restore must be a durable no-op after restart"
        );
        let mut conflict = restore;
        let WriteOp::RestoreCaBackup { ca_certs, .. } = &mut conflict else {
            unreachable!("restore fixture changed operation type")
        };
        ca_certs[0].ca_cert_pem = "ca:conflict".into();
        let refused = request(&socket, conflict).expect("typed conflict response");
        assert!(matches!(
            refused,
            WriteResponse::Error(error) if error.contains("conflicts with durable issuer")
        ));

        let reader = Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("reader");
        let durable_ca: String = reader
            .query_row(
                "SELECT ca_cert_pem FROM nebula_ca WHERE mesh_id = 'mesh-restore' AND epoch = 4",
                [],
                |row| row.get(0),
            )
            .expect("durable CA");
        let peer_rows: i64 = reader
            .query_row(
                "SELECT COUNT(*) FROM nebula_peer_certs WHERE node_id = 'peer:restore' AND epoch = 4",
                [],
                |row| row.get(0),
            )
            .expect("durable peer count");
        assert_eq!(durable_ca, "ca:4");
        assert_eq!(peer_rows, 1);
        let audit_rows = super::super::load_audit_rows(&reader).expect("audit rows");
        assert!(matches!(
            crate::audit::verify(&audit_rows),
            crate::audit::VerifyOutcome::Intact { verified: 1, .. }
        ));
        stop(server, &shutdown);
    }
}
