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
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
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
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WriteOp {
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
}

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
}

pub fn request_if_serving(operation: WriteOp) -> Result<Option<WriteResponse>> {
    let Some(socket) = SERVE_SOCKET.get() else {
        return Ok(None);
    };
    request(socket, operation).map(Some)
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
    let request: WriteRequest =
        serde_json::from_slice(payload).context("decoding write request")?;
    if request.schema_version != SCHEMA_VERSION {
        bail!(
            "unsupported SQLite writer schema {}; expected {}",
            request.schema_version,
            SCHEMA_VERSION
        );
    }
    execute(conn, request.operation)
}

fn execute(conn: &Connection, operation: WriteOp) -> Result<WriteResponse> {
    match operation {
        WriteOp::InsertEvent {
            kind,
            actor,
            payload_json,
        } => {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| {
                let prev_hash_hex: String = conn
                    .query_row("SELECT hash FROM events ORDER BY seq DESC LIMIT 1", [], |row| {
                        row.get(0)
                    })
                    .unwrap_or_default();
                let prev_bytes = super::decode_sha256_hex(&prev_hash_hex).unwrap_or([0; 32]);
                let now = chrono::Utc::now();
                let hash = crate::audit::next_hash(&prev_bytes, payload_json.as_bytes(), now.timestamp_millis());
                let hash_hex = super::encode_sha256_hex(&hash);
                conn.execute(
                    "INSERT INTO events (prev_hash, hash, kind, actor, payload_json, created_at) VALUES (?, ?, ?, ?, ?, ?)",
                    (&prev_hash_hex, &hash_hex, kind, actor, payload_json, now.to_rfc3339()),
                )?;
                Ok(WriteResponse::RowId(conn.last_insert_rowid()))
            })();
            finish_transaction(conn, result)
        }
        WriteOp::RollbackToRevision { target_id, new_id, author } => {
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
        WriteOp::SetNodeRole { node_id, role } => Ok(WriteResponse::Count(conn.execute(
            "UPDATE nodes SET role = ? WHERE node_id = ?",
            (role, node_id),
        )?)),
        WriteOp::SetNodeHealth { node_id, health } => {
            let prior: Option<String> = conn
                .query_row("SELECT health FROM nodes WHERE node_id = ?", [&node_id], |row| row.get(0))
                .optional()?;
            if prior.as_deref().is_none_or(|prior| prior == health) {
                return Ok(WriteResponse::Changed(false));
            }
            Ok(WriteResponse::Changed(conn.execute(
                "UPDATE nodes SET health = ? WHERE node_id = ?",
                (health, node_id),
            )? > 0))
        }
        WriteOp::SetNodeVersion { name, version } => Ok(WriteResponse::Changed(conn.execute(
            "UPDATE nodes SET mde_version = ? WHERE name = ?",
            (version, name),
        )? > 0)),
        WriteOp::RefreshNodeCredentials { node_id, new_public_key } => Ok(WriteResponse::Count(conn.execute(
            "UPDATE nodes SET public_key = ?, enrolled_at = ? WHERE node_id = ?",
            (new_public_key, chrono::Utc::now().to_rfc3339(), node_id),
        )?)),
        WriteOp::UpsertNode { node_id, name, public_key, region } => Ok(WriteResponse::Count(conn.execute(
            "INSERT INTO nodes (node_id, name, public_key, enrolled_at, region) VALUES (?, ?, ?, ?, ?) ON CONFLICT(node_id) DO UPDATE SET name = excluded.name, public_key = excluded.public_key, region = excluded.region",
            (node_id, name, public_key, chrono::Utc::now().to_rfc3339(), region),
        )?)),
    }
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

        let mut stream = UnixStream::connect(&socket).expect("connect oversized");
        stream
            .write_all(&u32::try_from(MAX_FRAME_BYTES + 1).unwrap().to_be_bytes())
            .expect("header");
        let response: WriteResponse =
            serde_json::from_slice(&read_frame(&mut stream).expect("response")).expect("decode");
        assert!(matches!(response, WriteResponse::Error(error) if error.contains("exceeds")));

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
}
