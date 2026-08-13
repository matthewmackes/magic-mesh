//! PRINT-2..PRINT-6 + PRINT-8 (event half) (v5.0.0) — auto CUPS print
//! sharing + sync worker.
//!
//! Runs on **headless + full** peers (lighthouse skips it at spawn).
//! Each 5 s tick converges the fleet's printers through the
//! `mesh-storage` volume the same write-own-file / read-union way the
//! PEERVER peer-data converges:
//!
//!   1. **Publish (PRINT-2).** Enumerate local queues (`lpstat`), write
//!      `<mesh-storage>/printers/<host>.json`; copy each legacy
//!      (non-IPP) queue's PPD to `<mesh-storage>/printers/ppd/<host>/`.
//!   2. **Share (PRINT-4).** Ensure `cupsd` listens on the **overlay IP
//!      only** (never `0.0.0.0`) + `cupsctl --share-printers` +
//!      per-queue `printer-is-shared=true`, so a job submitted on a
//!      remote peer reaches this host's hardware.
//!   3. **Import (PRINT-3).** Read the union of `printers/*.json`
//!      (minus self); `lpadmin` each remote queue as `<queue>@<host>`
//!      pointing at `ipp://<host-overlay>:631/printers/<queue>`
//!      (`-m everywhere`, or the replicated PPD for legacy). Prune
//!      local `<q>@<host>` queues whose host-file vanished.
//!   4. **Defaults (PRINT-5).** Reconcile `_defaults.json` (fleet
//!      default printer + per-queue presets) last-write-wins by
//!      `written_at_ms`; apply via `lpoptions`.
//!   5. **Auto-join (PRINT-6).** The periodic tick *is* the join: a
//!      newly-enrolled peer's `printers/<host>.json` becomes readable
//!      once `mesh-storage` mounts, and the next tick imports it (same
//!      polling-convergence model as `gluster_worker`).
//!   6. **Event (PRINT-8, event half).** On a local add/remove,
//!      publish `event/printers/<host>` on the Bus so panels refresh
//!      without polling. The `action/printers/{sync-now,list}` command
//!      surface is the PRINT-8.b follow-on.
//!
//! Silent no-op when `cupsd`/`lpadmin` aren't installed (the operator
//! hasn't opted into the print stack) or the overlay-ip publish file is
//! missing (peer hasn't completed Nebula enrollment) — exactly the
//! `gluster_worker` guard shape.
//!
//! Test surface: every decision is a pure function over parsed strings
//! / `serde_json::Value` (`parse_lpstat_e`, `parse_device_uri`,
//! `queue_kind`, `own_record`, `import_plan`, `lpadmin_add_argv`,
//! `prune_list`, `resolve_defaults_lww`, `cupsd_needs_listen`); the
//! worker body is a thin shell-out layer over them.

#![cfg(feature = "async-services")]

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mde_bus::hooks::Priority;
use mde_bus::persist::{Persist, StoredMessage};
use mde_bus::rpc::reply_topic;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

use super::nebula_supervisor::DEFAULT_OVERLAY_IP_PATH;
use super::{ShutdownToken, Worker};

/// PRINT-8.b — the two `action/printers/<verb>` topics this worker serves.
const ACTION_VERBS: [&str; 2] = ["sync-now", "list"];

/// Shared-Bus capability context for the printer-stack mutation. The action is
/// local to this node; the body is still bound exactly so a shared-spool writer
/// cannot turn `sync-now` into an unauthenticated CUPS/filesystem effect.
const CUPS_SYNC_AUTH_VERB: &str = "printers-sync-now";

/// Prompt initial retry for a late or temporarily unopenable Bus.
const MIN_BUS_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// Maximum retry delay while the Bus remains unavailable.
const MAX_BUS_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// Bound replies retained after an action effect completed but reply publication
/// failed. This is an in-memory same-worker barrier, not crash-durable state.
const MAX_PENDING_REPLIES: usize = 64;

const MAX_PROVIDER_OBSERVATION_BYTES: usize = 64 * 1024;
const MAX_PRINTER_FACTS: usize = 256;

/// Credential-free readiness of the local printer provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PrinterReadiness {
    Ready,
    Disconnected,
    Disabled,
    Unknown,
}

/// Bounded projection. Queue identities and command output never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PrinterProviderSnapshot {
    schema_version: u16,
    node_id: String,
    observed_unix_ms: u64,
    readiness: PrinterReadiness,
    configured_queues: u16,
    kernel_printers: u16,
    reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CupsServiceFact {
    loaded: bool,
    enabled: bool,
    active: bool,
}

fn parse_cups_service(raw: &str) -> Option<CupsServiceFact> {
    if raw.is_empty() || raw.len() > MAX_PROVIDER_OBSERVATION_BYTES || raw.contains('\0') {
        return None;
    }
    let mut fields = HashMap::new();
    for line in raw.lines() {
        let (key, value) = line.split_once('=')?;
        if !matches!(key, "LoadState" | "UnitFileState" | "ActiveState" | "SubState")
            || value.is_empty()
            || fields.insert(key, value).is_some()
        {
            return None;
        }
    }
    if fields.len() != 4 {
        return None;
    }
    let loaded = match fields["LoadState"] {
        "loaded" => true,
        "not-found" => false,
        _ => return None,
    };
    let enabled = match fields["UnitFileState"] {
        "enabled" | "enabled-runtime" | "static" | "indirect" => true,
        "disabled" | "masked" | "not-found" => false,
        _ => return None,
    };
    let active = match (fields["ActiveState"], fields["SubState"]) {
        ("active", "running") => true,
        ("inactive" | "failed", "dead" | "failed") => false,
        _ => return None,
    };
    Some(CupsServiceFact {
        loaded,
        enabled,
        active,
    })
}

fn parse_scheduler(raw: &str) -> Option<bool> {
    if raw.len() > MAX_PROVIDER_OBSERVATION_BYTES || raw.contains('\0') {
        return None;
    }
    match raw.trim() {
        "scheduler is running" => Some(true),
        "scheduler is not running" => Some(false),
        _ => None,
    }
}

fn parse_provider_queues(raw: &str) -> Option<usize> {
    if raw.len() > MAX_PROVIDER_OBSERVATION_BYTES || raw.contains('\0') {
        return None;
    }
    let mut queues = BTreeSet::new();
    for name in raw.lines().map(str::trim).filter(|name| !name.is_empty()) {
        if name.len() > 127
            || !name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'@')
            })
            || !queues.insert(name)
            || queues.len() > MAX_PRINTER_FACTS
        {
            return None;
        }
    }
    Some(queues.len())
}

fn classify_printer_provider(
    service: Option<CupsServiceFact>,
    scheduler: Option<bool>,
    queues: Option<usize>,
    kernel_printers: Option<Vec<String>>,
) -> (PrinterReadiness, usize, usize, &'static str) {
    let (Some(service), Some(scheduler), Some(queue_count), Some(mut kernel)) =
        (service, scheduler, queues, kernel_printers)
    else {
        return (PrinterReadiness::Unknown, 0, 0, "printer facts unavailable or malformed");
    };
    if kernel.len() > MAX_PRINTER_FACTS
        || kernel.iter().any(|name| {
            !name.strip_prefix("lp").is_some_and(|index| {
                !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
    {
        return (PrinterReadiness::Unknown, 0, 0, "kernel printer inventory malformed");
    }
    kernel.sort_unstable();
    if kernel.windows(2).any(|pair| pair[0] == pair[1]) {
        return (PrinterReadiness::Unknown, 0, 0, "kernel printer identities are duplicated");
    }
    if service.active != scheduler
        || (!service.loaded && (service.enabled || service.active))
        || (!service.active && queue_count > 0)
    {
        return (PrinterReadiness::Unknown, 0, 0, "CUPS and system facts contradict");
    }
    if !service.loaded || (!service.enabled && !service.active) {
        return (
            PrinterReadiness::Disabled,
            queue_count,
            kernel.len(),
            "CUPS is not enabled",
        );
    }
    if service.active && queue_count == 0 {
        return (
            PrinterReadiness::Disconnected,
            0,
            kernel.len(),
            "CUPS is running without a configured queue",
        );
    }
    if service.active {
        return (
            PrinterReadiness::Ready,
            queue_count,
            kernel.len(),
            "CUPS service, scheduler, and queue facts agree",
        );
    }
    (
        PrinterReadiness::Disconnected,
        queue_count,
        kernel.len(),
        "CUPS is enabled but not running",
    )
}

/// Tick cadence — five seconds, matching the other mesh workers.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(5);

/// IPP port `cupsd` serves on (the host's overlay endpoint).
pub const IPP_PORT: u16 = 631;

fn action_topic(verb: &str) -> String {
    format!("action/printers/{verb}")
}

trait CupsBusFactory: Send + Sync {
    fn open(&self, root: &std::path::Path) -> Result<Option<Persist>, String>;
}

struct PersistCupsBusFactory;

impl CupsBusFactory for PersistCupsBusFactory {
    fn open(&self, root: &std::path::Path) -> Result<Option<Persist>, String> {
        Persist::open(root.to_path_buf())
            .map(Some)
            .map_err(|error| error.to_string())
    }
}

trait ActionLaneReader: Send + Sync {
    fn read(
        &self,
        persist: &Persist,
        topic: &str,
        since: Option<&str>,
    ) -> Result<Vec<StoredMessage>, String>;
}

struct PersistActionLaneReader;

impl ActionLaneReader for PersistActionLaneReader {
    fn read(
        &self,
        persist: &Persist,
        topic: &str,
        since: Option<&str>,
    ) -> Result<Vec<StoredMessage>, String> {
        persist
            .list_since(topic, since)
            .map_err(|error| error.to_string())
    }
}

trait ReplyWriter: Send + Sync {
    fn write(&self, persist: &Persist, request_ulid: &str, body: &str) -> Result<(), String>;
}

struct PersistReplyWriter;

impl ReplyWriter for PersistReplyWriter {
    fn write(&self, persist: &Persist, request_ulid: &str, body: &str) -> Result<(), String> {
        persist
            .write(
                &reply_topic(request_ulid),
                Priority::Default,
                None,
                Some(body),
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
struct PendingReply {
    topic: String,
    cursor: String,
    body: String,
}

struct StagedActions {
    lanes: Vec<(String, String, Vec<StoredMessage>)>,
}

/// Whether a queue is a modern IPP-Everywhere printer (no PPD needed)
/// or a legacy queue whose PPD must replicate (Q4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueKind {
    /// Driverless IPP Everywhere — importing peers use `-m everywhere`.
    Everywhere,
    /// Legacy — the host's PPD replicates so importers present options.
    Ppd,
}

impl QueueKind {
    /// Wire string written into `printers/<host>.json`'s `kind` field.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Everywhere => "everywhere",
            Self::Ppd => "ppd",
        }
    }
}

// ───────────────────────── pure helpers ─────────────────────────

/// Parse `lpstat -e` (one queue name per line) into queue names.
#[must_use]
pub fn parse_lpstat_e(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Extract the device URI for `queue` from `lpstat -v` output
/// (`device for <queue>: <uri>`).
#[must_use]
pub fn parse_device_uri(lpstat_v: &str, queue: &str) -> Option<String> {
    let needle = format!("device for {queue}:");
    lpstat_v
        .lines()
        .find_map(|l| l.trim().strip_prefix(&needle).map(|u| u.trim().to_string()))
}

/// Classify a queue from its device URI: an `ipp`/`ipps`/`dnssd` URI is
/// a driverless IPP-Everywhere candidate; everything else (usb, socket,
/// parallel, lpd, …) is legacy and replicates its PPD.
#[must_use]
pub fn queue_kind(device_uri: &str) -> QueueKind {
    let u = device_uri.trim_start();
    if u.starts_with("ipp://") || u.starts_with("ipps://") || u.starts_with("dnssd://") {
        QueueKind::Everywhere
    } else {
        QueueKind::Ppd
    }
}

/// Build this peer's `printers/<host>.json` record.
#[must_use]
pub fn own_record(
    host: &str,
    overlay_ip: &str,
    queues: &[(String, QueueKind)],
    now_ms: u64,
) -> Value {
    let q: Vec<Value> = queues
        .iter()
        .map(|(name, kind)| {
            json!({
                "name": name,
                "kind": kind.as_str(),
                "ipp_path": format!("ipp://{overlay_ip}:{IPP_PORT}/printers/{name}"),
            })
        })
        .collect();
    json!({
        "host": host,
        "overlay_ip": overlay_ip,
        "queues": q,
        "written_at_ms": now_ms,
    })
}

/// A remote queue to import locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportQueue {
    /// Local name: `<queue>@<host>`.
    pub local_name: String,
    /// `ipp://<host-overlay>:631/printers/<queue>`.
    pub uri: String,
    /// Driverless (`everywhere`) vs legacy (replicated PPD).
    pub kind: QueueKind,
    /// Source host (for the replicated-PPD lookup when `kind == Ppd`).
    pub host: String,
    /// Original queue name on the host.
    pub queue: String,
}

/// From the union of peer records (minus self), compute the remote
/// queues to import. `self_host` is excluded.
#[must_use]
pub fn import_plan(self_host: &str, records: &[Value]) -> Vec<ImportQueue> {
    let mut out = Vec::new();
    for rec in records {
        let Some(host) = rec.get("host").and_then(Value::as_str) else {
            continue;
        };
        if host == self_host {
            continue;
        }
        let Some(queues) = rec.get("queues").and_then(Value::as_array) else {
            continue;
        };
        for q in queues {
            let (Some(name), Some(uri)) = (
                q.get("name").and_then(Value::as_str),
                q.get("ipp_path").and_then(Value::as_str),
            ) else {
                continue;
            };
            let kind = match q.get("kind").and_then(Value::as_str) {
                Some("ppd") => QueueKind::Ppd,
                _ => QueueKind::Everywhere,
            };
            out.push(ImportQueue {
                local_name: format!("{name}@{host}"),
                uri: uri.to_string(),
                kind,
                host: host.to_string(),
                queue: name.to_string(),
            });
        }
    }
    out.sort_by(|a, b| a.local_name.cmp(&b.local_name));
    out
}

/// `lpadmin` argv to add/refresh an imported remote queue. Legacy
/// queues get the replicated PPD path (`-P`); everywhere queues use the
/// driverless model (`-m everywhere`).
#[must_use]
pub fn lpadmin_add_argv(q: &ImportQueue, ppd_path: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "-p".to_string(),
        q.local_name.clone(),
        "-E".to_string(),
        "-v".to_string(),
        q.uri.clone(),
    ];
    match (q.kind, ppd_path) {
        (QueueKind::Ppd, Some(p)) => {
            argv.push("-P".to_string());
            argv.push(p.to_string());
        }
        _ => {
            argv.push("-m".to_string());
            argv.push("everywhere".to_string());
        }
    }
    argv
}

/// Local `<q>@<host>` queues to delete: any currently-present imported
/// queue not in the desired import set (the host file vanished or the
/// queue was removed upstream). Only `@`-bearing names are candidates —
/// the peer's own local queues are never pruned.
#[must_use]
pub fn prune_list(existing_local: &[String], desired_import: &[String]) -> Vec<String> {
    let desired: BTreeSet<&str> = desired_import.iter().map(String::as_str).collect();
    existing_local
        .iter()
        .filter(|n| n.contains('@') && !desired.contains(n.as_str()))
        .cloned()
        .collect()
}

/// Resolve the fleet defaults record from all peers' `_defaults.json`
/// fragments: highest `written_at_ms` wins (LWW, Q5). Returns `None`
/// when there are no records.
#[must_use]
pub fn resolve_defaults_lww(records: &[Value]) -> Option<Value> {
    records
        .iter()
        .max_by_key(|r| r.get("written_at_ms").and_then(Value::as_u64).unwrap_or(0))
        .cloned()
}

/// Does `cupsd.conf` already listen on the overlay IP? (idempotence
/// guard so the worker only rewrites + reloads on a real change).
#[must_use]
pub fn cupsd_needs_listen(cupsd_conf: &str, overlay_ip: &str) -> bool {
    let listen = format!("Listen {overlay_ip}:{IPP_PORT}");
    !cupsd_conf.lines().any(|l| l.trim() == listen)
}

/// Add the overlay-only `Listen` + an overlay-CIDR `<Location />` allow
/// to a `cupsd.conf`. Binds the Nebula overlay interface only — never
/// `0.0.0.0` (open-mesh directive + §0.7 #10 public-port lint).
#[must_use]
pub fn cupsd_with_listen(cupsd_conf: &str, overlay_ip: &str, overlay_cidr: &str) -> String {
    let block = format!(
        "\n# PRINT-4 (mde cups_sync): share local printers on the Nebula\n\
         # overlay ONLY. Never 0.0.0.0 — enrolled peers reach this via the\n\
         # tunnel; the single mesh passcode is the auth boundary.\n\
         Listen {overlay_ip}:{IPP_PORT}\n\
         <Location />\n  Order allow,deny\n  Allow from {overlay_cidr}\n</Location>\n"
    );
    let mut s = cupsd_conf.to_string();
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&block);
    s
}

// ───────────────────────── worker body ─────────────────────────

/// Auto CUPS print-sharing worker. One per headless/full peer.
pub struct CupsSyncWorker {
    tick: Duration,
    mesh_home: PathBuf,
    overlay_ip_path: PathBuf,
    hostname: String,
    /// Overlay CIDR allowed in the cupsd `<Location>` (the mesh subnet).
    overlay_cidr: String,
    /// Shelled binaries (injectable for tests).
    lpstat: String,
    lpadmin: String,
    lpoptions: String,
    cupsctl: String,
    /// Explicit Bus root override. Production resolves this at worker startup
    /// and falls back to the canonical system spool when user resolution is
    /// absent; `None` therefore never permanently disables actions.
    bus_root_override: Option<PathBuf>,
    /// Per-verb read cursors for the `action/printers/<verb>` topics.
    action_cursors: HashMap<String, String>,
    bus_factory: Arc<dyn CupsBusFactory>,
    action_reader: Arc<dyn ActionLaneReader>,
    reply_writer: Arc<dyn ReplyWriter>,
    /// Completed action replies awaiting publication. This blocks duplicate
    /// effects only within this process; a crash can lose the barrier.
    pending_replies: HashMap<String, PendingReply>,
    /// Shared, fail-closed authorization gate for `sync-now`.
    authorizer: Arc<ActionAuthorizer>,
    #[cfg(test)]
    action_effect: Option<Arc<dyn Fn(&str) -> String + Send + Sync>>,
}

impl CupsSyncWorker {
    /// Construct with production defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tick: DEFAULT_TICK_INTERVAL,
            mesh_home: mackes_mesh_types::peers::default_mesh_home(),
            overlay_ip_path: PathBuf::from(DEFAULT_OVERLAY_IP_PATH),
            hostname: local_hostname(),
            overlay_cidr: "10.42.0.0/16".to_string(),
            lpstat: "lpstat".to_string(),
            lpadmin: "lpadmin".to_string(),
            lpoptions: "lpoptions".to_string(),
            cupsctl: "cupsctl".to_string(),
            bus_root_override: None,
            action_cursors: HashMap::new(),
            bus_factory: Arc::new(PersistCupsBusFactory),
            action_reader: Arc::new(PersistActionLaneReader),
            reply_writer: Arc::new(PersistReplyWriter),
            pending_replies: HashMap::new(),
            authorizer: Arc::new(ActionAuthorizer::production()),
            #[cfg(test)]
            action_effect: None,
        }
    }

    /// Inject an isolated verifier and replay ledger for hostile action tests.
    /// Production always uses the systemd-credential-backed authorizer.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_bus_factory(mut self, factory: Arc<dyn CupsBusFactory>) -> Self {
        self.bus_factory = factory;
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_action_reader(mut self, reader: Arc<dyn ActionLaneReader>) -> Self {
        self.action_reader = reader;
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_reply_writer(mut self, writer: Arc<dyn ReplyWriter>) -> Self {
        self.reply_writer = writer;
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_action_effect(mut self, effect: Arc<dyn Fn(&str) -> String + Send + Sync>) -> Self {
        self.action_effect = Some(effect);
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    fn printers_dir(&self) -> PathBuf {
        self.mesh_home.join("printers")
    }

    fn printer_provider_path(&self) -> PathBuf {
        self.mesh_home
            .join("printer-provider")
            .join(format!("{}.json", self.hostname))
    }

    fn provider_capture(&self, bin: &str, args: &[&str]) -> Option<String> {
        let mut command = Command::new(bin);
        command.env("LC_ALL", "C").args(args);
        let output = crate::workers::proc::output_with_timeout(
            command,
            crate::workers::proc::DEFAULT_CMD_TIMEOUT,
        )
        .ok()?;
        if !output.status.success() || output.stdout.len() > MAX_PROVIDER_OBSERVATION_BYTES {
            return None;
        }
        String::from_utf8(output.stdout).ok()
    }

    fn kernel_printers(root: &Path) -> Option<Vec<String>> {
        let entries = match std::fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
            Err(_) => return None,
        };
        let mut printers = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with("lp"))
            .collect::<Vec<_>>();
        if printers.len() > MAX_PRINTER_FACTS {
            printers.truncate(MAX_PRINTER_FACTS + 1);
        }
        Some(printers)
    }

    /// Publish only bounded readiness and counts; this adds no printer mutation path.
    fn publish_provider_readiness(&self) {
        let service = self
            .provider_capture(
                "systemctl",
                &[
                    "show",
                    "cups.service",
                    "--property=LoadState",
                    "--property=UnitFileState",
                    "--property=ActiveState",
                    "--property=SubState",
                    "--no-pager",
                ],
            )
            .as_deref()
            .and_then(parse_cups_service);
        let disabled = service.is_some_and(|fact| !fact.loaded);
        let scheduler = if disabled {
            Some(false)
        } else {
            self.provider_capture(&self.lpstat, &["-r"])
                .as_deref()
                .and_then(parse_scheduler)
        };
        let queues = if disabled {
            Some(0)
        } else {
            self.provider_capture(&self.lpstat, &["-e"])
                .as_deref()
                .and_then(parse_provider_queues)
        };
        let kernel = Self::kernel_printers(Path::new("/sys/class/usb"));
        let (readiness, queue_count, kernel_count, reason) =
            classify_printer_provider(service, scheduler, queues, kernel);
        let snapshot = PrinterProviderSnapshot {
            schema_version: 1,
            node_id: self.hostname.clone(),
            observed_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            readiness,
            configured_queues: queue_count.try_into().unwrap_or(u16::MAX),
            kernel_printers: kernel_count.try_into().unwrap_or(u16::MAX),
            reason: reason.to_owned(),
        };
        if self.hostname.is_empty()
            || self.hostname.len() > 128
            || !self
                .hostname
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return;
        }
        let path = self.printer_provider_path();
        let Some(parent) = path.parent() else { return };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let temporary = parent.join(format!(".{}.json.tmp", self.hostname));
        if serde_json::to_vec_pretty(&snapshot)
            .ok()
            .is_some_and(|bytes| std::fs::write(&temporary, bytes).is_ok())
        {
            let _ = std::fs::rename(temporary, path);
        }
    }

    /// One tick. Guarded no-op when cups/lpadmin absent or unenrolled.
    fn tick_once(&self) {
        if which(&self.lpstat).is_none() || which(&self.lpadmin).is_none() {
            return; // print stack not installed — operator hasn't opted in.
        }
        let Some(overlay_ip) = self.read_overlay_ip() else {
            return; // not enrolled yet — no stable overlay endpoint.
        };
        let dir = self.printers_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return; // mesh-storage not mounted/writable yet.
        }

        // 1. Publish local queues + PPDs (PRINT-2).
        let local = self.local_queues();
        let changed = self.publish_own(&dir, &overlay_ip, &local);

        // 2. Ensure host-side sharing (PRINT-4).
        self.ensure_sharing(&overlay_ip, &local);

        // 3. Import the union + prune (PRINT-3).
        let union = read_peer_records(&dir);
        let plan = import_plan(&self.hostname, &union);
        self.apply_imports(&dir, &plan);
        let desired: Vec<String> = plan.iter().map(|q| q.local_name.clone()).collect();
        for stale in prune_list(&self.installed_queue_names(), &desired) {
            let _ = self.run_lpadmin(&["-x", &stale]);
        }

        // 4. Reconcile defaults + presets LWW (PRINT-5).
        self.reconcile_defaults(&dir);

        // 5. Event publish on a local change (PRINT-8, event half).
        if changed {
            self.publish_event();
        }
    }

    fn read_overlay_ip(&self) -> Option<String> {
        std::fs::read_to_string(&self.overlay_ip_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Local queues with their kind (everywhere vs legacy-PPD).
    fn local_queues(&self) -> Vec<(String, QueueKind)> {
        let names = match self.run_capture(&self.lpstat, &["-e"]) {
            Some(out) => parse_lpstat_e(&out),
            None => return Vec::new(),
        };
        // Drop already-imported `@host` queues — only our own hardware.
        let names: Vec<String> = names.into_iter().filter(|n| !n.contains('@')).collect();
        let uris = self.run_capture(&self.lpstat, &["-v"]).unwrap_or_default();
        names
            .into_iter()
            .map(|n| {
                let kind = parse_device_uri(&uris, &n).map_or(QueueKind::Ppd, |u| queue_kind(&u));
                (n, kind)
            })
            .collect()
    }

    /// Write `printers/<host>.json` + replicate legacy PPDs. Returns
    /// whether the record changed since last tick (drives the event).
    fn publish_own(
        &self,
        dir: &std::path::Path,
        overlay_ip: &str,
        local: &[(String, QueueKind)],
    ) -> bool {
        let rec = own_record(&self.hostname, overlay_ip, local, now_ms());
        let path = dir.join(format!("{}.json", self.hostname));
        let queues_now = serde_json::to_string(&rec["queues"]).unwrap_or_default();
        let prev = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .map(|v| serde_json::to_string(&v["queues"]).unwrap_or_default())
            .unwrap_or_default();
        let changed = queues_now != prev;
        if let Ok(json) = serde_json::to_string_pretty(&rec) {
            let _ = std::fs::write(&path, json);
        }
        // Replicate each legacy queue's PPD (Q4).
        let ppd_dir = dir.join("ppd").join(&self.hostname);
        let _ = std::fs::create_dir_all(&ppd_dir);
        for (name, kind) in local {
            if *kind == QueueKind::Ppd {
                let src = PathBuf::from(format!("/etc/cups/ppd/{name}.ppd"));
                if src.exists() {
                    let _ = std::fs::copy(&src, ppd_dir.join(format!("{name}.ppd")));
                }
            }
        }
        changed
    }

    /// Ensure cupsd listens on the overlay + shares each local queue.
    fn ensure_sharing(&self, overlay_ip: &str, local: &[(String, QueueKind)]) {
        let conf = PathBuf::from("/etc/cups/cupsd.conf");
        if let Ok(text) = std::fs::read_to_string(&conf) {
            if cupsd_needs_listen(&text, overlay_ip) {
                let next = cupsd_with_listen(&text, overlay_ip, &self.overlay_cidr);
                if std::fs::write(&conf, next).is_ok() {
                    // EFF-20 — bound systemctl reload.
                    let mut cmd = Command::new("systemctl");
                    cmd.args(["reload", "cups.service"]);
                    let _ = crate::workers::proc::status_with_timeout(
                        cmd,
                        crate::workers::proc::DEFAULT_CMD_TIMEOUT,
                    );
                }
            }
        }
        let _ = self.run_capture(&self.cupsctl, &["--share-printers"]);
        for (name, _) in local {
            let _ = self.run_lpadmin(&["-p", name, "-o", "printer-is-shared=true"]);
        }
    }

    fn apply_imports(&self, dir: &std::path::Path, plan: &[ImportQueue]) {
        for q in plan {
            let ppd = if q.kind == QueueKind::Ppd {
                let p = dir
                    .join("ppd")
                    .join(&q.host)
                    .join(format!("{}.ppd", q.queue));
                p.exists().then(|| p.to_string_lossy().to_string())
            } else {
                None
            };
            let argv = lpadmin_add_argv(q, ppd.as_deref());
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            let _ = self.run_lpadmin(&refs);
        }
    }

    fn reconcile_defaults(&self, dir: &std::path::Path) {
        let path = dir.join("_defaults.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        // The file holds a single object today; LWW is across historical
        // writers via `written_at_ms` once multiple peers contend.
        let Ok(rec) = serde_json::from_str::<Value>(&text) else {
            return;
        };
        let winner = resolve_defaults_lww(std::slice::from_ref(&rec));
        if let Some(d) = winner {
            if let Some(def) = d.get("default_printer").and_then(Value::as_str) {
                // EFF-20 — bound lpoptions.
                let mut cmd = Command::new(&self.lpoptions);
                cmd.args(["-d", def]);
                let _ = crate::workers::proc::status_with_timeout(
                    cmd,
                    crate::workers::proc::DEFAULT_CMD_TIMEOUT,
                );
            }
        }
    }

    fn installed_queue_names(&self) -> Vec<String> {
        self.run_capture(&self.lpstat, &["-e"])
            .map(|o| parse_lpstat_e(&o))
            .unwrap_or_default()
    }

    /// PRINT-8 — announce a local printer change on `event/printers/<host>`
    /// in-process (perf-10 / arch-6). Replaces a raw `mde-bus publish … .spawn()`
    /// (a fork+exec + fresh SQLite open that was never even reaped) with a bare
    /// `Persist::write`. Byte-identical stored row; targets
    /// [`crate::bus_publish::default_bus_root`] (honours `MDE_BUS_ROOT` — the
    /// root the spawned CLI resolved via the inherited env). Best-effort.
    fn publish_event(&self) {
        let topic = format!("event/printers/{}", self.hostname);
        let body = format!(r#"{{"host":"{}","changed":true}}"#, self.hostname);
        if let Some(mut persist) =
            crate::bus_publish::open_bus(crate::bus_publish::default_bus_root())
        {
            crate::bus_publish::publish_body(&mut persist, &topic, &body);
        }
    }

    /// Tail-prime both transient action lanes as one activation transaction.
    /// Candidate cursors are returned only after every full tail read succeeds,
    /// so retained sync/list commands can never partially replay after restart.
    fn prime_action_cursors(&self, persist: &Persist) -> Result<HashMap<String, String>, String> {
        let mut candidate = HashMap::new();
        for verb in ACTION_VERBS {
            let topic = action_topic(verb);
            let messages = self
                .action_reader
                .read(persist, &topic, None)
                .map_err(|error| format!("tail-prime {topic}: {error}"))?;
            if let Some(last) = messages.last() {
                candidate.insert(topic, last.ulid.clone());
            }
        }
        Ok(candidate)
    }

    /// Read both lanes completely before returning any work. The caller performs
    /// no effect and changes no cursor when either lane fails.
    fn stage_actions(&self, persist: &Persist) -> Result<StagedActions, String> {
        let mut lanes = Vec::with_capacity(ACTION_VERBS.len());
        for verb in ACTION_VERBS {
            let topic = action_topic(verb);
            let since = self.action_cursors.get(&topic).map(String::as_str);
            let messages = self
                .action_reader
                .read(persist, &topic, since)
                .map_err(|error| format!("read {topic}: {error}"))?;
            lanes.push((verb.to_string(), topic, messages));
        }
        Ok(StagedActions { lanes })
    }

    /// Process a fully staged sweep. Reply publication is the commit boundary:
    /// the cursor advances only after the required reply write succeeds.
    /// Completed replies are held in a bounded in-memory same-worker barrier so
    /// a transient reply failure does not repeat a sync effect. A process crash
    /// after the effect but before durable reply publication can still replay it.
    fn process_staged_actions(
        &mut self,
        persist: &Persist,
        staged: StagedActions,
    ) -> Result<(), String> {
        for (verb, topic, messages) in staged.lanes {
            for message in messages {
                if let Some(pending) = self.pending_replies.get(&message.ulid).cloned() {
                    self.reply_writer
                        .write(persist, &message.ulid, &pending.body)
                        .map_err(|error| format!("retry reply {}: {error}", message.ulid))?;
                    self.action_cursors
                        .insert(pending.topic, pending.cursor.clone());
                    self.pending_replies.remove(&message.ulid);
                    continue;
                }

                if self.pending_replies.len() >= MAX_PENDING_REPLIES {
                    return Err(format!(
                        "pending reply barrier reached {MAX_PENDING_REPLIES}; deferring effects"
                    ));
                }

                let reply = self.handle_bus_action(&verb, message.body.as_deref());
                // Install the same-process barrier immediately after dispatch and
                // before publication. This state is intentionally not crash-safe.
                self.pending_replies.insert(
                    message.ulid.clone(),
                    PendingReply {
                        topic: topic.clone(),
                        cursor: message.ulid.clone(),
                        body: reply.clone(),
                    },
                );
                self.reply_writer
                    .write(persist, &message.ulid, &reply)
                    .map_err(|error| format!("write required reply {}: {error}", message.ulid))?;
                self.action_cursors
                    .insert(topic.clone(), message.ulid.clone());
                self.pending_replies.remove(&message.ulid);
            }
        }
        Ok(())
    }

    fn poll_bus_actions(&mut self, persist: &Persist) -> Result<(), String> {
        let staged = self.stage_actions(persist)?;
        self.process_staged_actions(persist, staged)
    }

    /// Dispatch one Bus action. Read-only listing stays open; the sync lane
    /// must authenticate before it reaches [`Self::handle_action`], which owns
    /// the CUPS/filesystem side effects.
    #[must_use]
    fn handle_bus_action(&self, verb: &str, body: Option<&str>) -> String {
        if verb == "sync-now" {
            let result = body
                .ok_or_else(|| "sync-now requires an authenticated request body".to_string())
                .and_then(|body| {
                    self.authorizer.authorize(
                        body,
                        MutationContext {
                            verb: CUPS_SYNC_AUTH_VERB,
                            node: &self.hostname,
                            target: &self.hostname,
                        },
                    )
                });
            if let Err(error) = result {
                tracing::warn!(
                    target: "mackesd::action_auth",
                    node = %self.hostname,
                    %error,
                    "cups_sync: refused unauthorized sync-now request"
                );
                return json!({ "error": format!("sync-now: authorization refused: {error}") })
                    .to_string();
            }
        }
        self.handle_action(verb)
    }

    /// Dispatch one `action/printers/<verb>` message. Returns the JSON
    /// body to write to `reply/<ulid>`. This is called only after the Bus
    /// mutation gate above; the periodic local tick is an explicit daemon-owned
    /// authority path and does not consume a Bus capability.
    #[must_use]
    fn handle_action(&self, verb: &str) -> String {
        #[cfg(test)]
        if let Some(effect) = &self.action_effect {
            return effect(verb);
        }

        match verb {
            "sync-now" => {
                self.tick_once();
                r#"{"ok":true}"#.to_string()
            }
            "list" => {
                let union = read_peer_records(&self.printers_dir());
                serde_json::to_string(&union).unwrap_or_else(|_| "[]".to_string())
            }
            _ => r#"{"error":"unknown verb"}"#.to_string(),
        }
    }

    fn run_lpadmin(&self, args: &[&str]) -> bool {
        // EFF-20 — bound lpadmin so a wedged CUPS can't pin the tick.
        let mut cmd = Command::new(&self.lpadmin);
        cmd.args(args);
        crate::workers::proc::status_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn run_capture(&self, bin: &str, args: &[&str]) -> Option<String> {
        // EFF-20 — bound the capture so a wedged CUPS tool can't pin the tick.
        let mut cmd = Command::new(bin);
        cmd.args(args);
        crate::workers::proc::output_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    }
}

impl Default for CupsSyncWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for CupsSyncWorker {
    fn name(&self) -> &'static str {
        "cups_sync"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = cups_sync_bus_root(self.bus_root_override.clone());
        // A statically absent optional CUPS stack has no periodic CUPS timer or
        // filesystem/subprocess convergence. Bus list/actions remain available;
        // an explicit sync request still performs the existing guarded no-op.
        let cups_enabled = which(&self.lpstat).is_some() && which(&self.lpadmin).is_some();
        self.publish_provider_readiness();
        if cups_enabled {
            self.tick_once();
        }
        let mut cups_tick = tokio::time::interval(self.tick);
        cups_tick.tick().await; // burn the immediate interval tick

        let mut activated = false;
        let mut retry_interval = MIN_BUS_RETRY_INTERVAL;
        loop {
            let bus_ready = match self.bus_factory.open(&bus_root) {
                Ok(Some(persist)) if activated => match self.poll_bus_actions(&persist) {
                    Ok(()) => true,
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            "cups_sync: complete action sweep deferred"
                        );
                        false
                    }
                },
                Ok(Some(persist)) => match self.prime_action_cursors(&persist) {
                    Ok(cursors) => {
                        self.action_cursors = cursors;
                        activated = true;
                        true
                    }
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            "cups_sync: atomic action tail-prime failed; activation will retry"
                        );
                        false
                    }
                },
                Ok(None) => {
                    tracing::debug!("cups_sync: Bus unavailable; action recovery will retry");
                    false
                }
                Err(error) => {
                    tracing::warn!(%error, "cups_sync: Bus open failed; action recovery will retry");
                    false
                }
            };
            let bus_delay = if bus_ready {
                retry_interval = MIN_BUS_RETRY_INTERVAL;
                self.tick
            } else {
                let delay = retry_interval;
                retry_interval = next_bus_retry_interval(retry_interval);
                delay
            };

            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(bus_delay) => {}
                _ = cups_tick.tick() => {
                    self.publish_provider_readiness();
                    if cups_enabled {
                        self.tick_once();
                    }
                },
            }
        }
    }
}

fn which(bin: &str) -> Option<PathBuf> {
    // Absolute path → check directly; bare name → scan PATH.
    let p = PathBuf::from(bin);
    if p.is_absolute() {
        return p.exists().then_some(p);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(bin))
            .find(|c| c.exists())
    })
}

fn local_hostname() -> String {
    let cmd = Command::new("hostname");
    crate::workers::proc::output_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

fn read_peer_records(dir: &std::path::Path) -> Vec<Value> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|x| x == "json")
                && p.file_name().is_some_and(|n| n != "_defaults.json")
            {
                if let Ok(v) = std::fs::read_to_string(&p).and_then(|s| {
                    serde_json::from_str::<Value>(&s)
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                }) {
                    out.push(v);
                }
            }
        }
    }
    out
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn cups_sync_bus_root(override_root: Option<PathBuf>) -> PathBuf {
    cups_sync_bus_root_or_system(override_root.or_else(mde_bus::default_data_dir))
}

fn cups_sync_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn next_bus_retry_interval(current: Duration) -> Duration {
    current
        .saturating_mul(2)
        .clamp(MIN_BUS_RETRY_INTERVAL, MAX_BUS_RETRY_INTERVAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    const AUTH_KEY: &[u8] = b"cups-sync-action-auth-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    #[test]
    fn hostile_printer_provider_facts_fail_unknown_without_exposing_identifiers() {
        let service = Some(CupsServiceFact {
            loaded: true,
            enabled: true,
            active: true,
        });
        let oversized = "x".repeat(MAX_PROVIDER_OBSERVATION_BYTES + 1);
        assert!(parse_cups_service(
            "LoadState=loaded\nLoadState=loaded\nUnitFileState=enabled\nActiveState=active\nSubState=running\n"
        )
        .is_none());
        assert!(parse_cups_service(
            "LoadState=loaded\nUnitFileState=enabled\nActiveState=active\nSubState=substituted\n"
        )
        .is_none());
        assert!(parse_provider_queues("office-secret\noffice-secret\n").is_none());
        assert!(parse_provider_queues(&oversized).is_none());

        let cases = [
            classify_printer_provider(service, Some(false), Some(1), Some(vec!["lp0".into()])),
            classify_printer_provider(service, Some(true), None, Some(vec!["lp0".into()])),
            classify_printer_provider(
                service,
                Some(true),
                Some(1),
                Some(vec!["lp0".into(), "lp0".into()]),
            ),
            classify_printer_provider(
                service,
                Some(true),
                Some(1),
                Some(vec!["lp-secret".into()]),
            ),
        ];
        for (readiness, queues, kernel, reason) in cases {
            assert_eq!(readiness, PrinterReadiness::Unknown);
            assert_eq!((queues, kernel), (0, 0));
            assert!(!reason.contains("office-secret"));
            assert!(!reason.contains("lp-secret"));
        }
    }

    #[test]
    fn printer_provider_distinguishes_ready_disconnected_and_disabled() {
        let active = Some(CupsServiceFact {
            loaded: true,
            enabled: true,
            active: true,
        });
        assert_eq!(
            classify_printer_provider(active, Some(true), Some(1), Some(vec![])).0,
            PrinterReadiness::Ready
        );
        assert_eq!(
            classify_printer_provider(active, Some(true), Some(0), Some(vec!["lp0".into()])).0,
            PrinterReadiness::Disconnected
        );
        assert_eq!(
            classify_printer_provider(
                Some(CupsServiceFact {
                    loaded: false,
                    enabled: false,
                    active: false,
                }),
                Some(false),
                Some(0),
                Some(vec![]),
            )
            .0,
            PrinterReadiness::Disabled
        );
    }

    struct LateBusFactory {
        root: PathBuf,
        attempts: Arc<AtomicUsize>,
    }

    impl CupsBusFactory for LateBusFactory {
        fn open(&self, _root: &std::path::Path) -> Result<Option<Persist>, String> {
            match self.attempts.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(None),
                1 => Err("injected unopenable Bus".into()),
                _ => Persist::open(self.root.clone())
                    .map(Some)
                    .map_err(|error| error.to_string()),
            }
        }
    }

    struct UnavailableBusFactory {
        attempts: Arc<AtomicUsize>,
    }

    impl CupsBusFactory for UnavailableBusFactory {
        fn open(&self, _root: &std::path::Path) -> Result<Option<Persist>, String> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    }

    #[derive(Default)]
    struct HostileLaneReader {
        fail_topic: Mutex<Option<String>>,
        reads: AtomicUsize,
    }

    impl HostileLaneReader {
        fn fail_next(&self, topic: &str) {
            *self.fail_topic.lock().unwrap() = Some(topic.to_string());
        }
    }

    impl ActionLaneReader for HostileLaneReader {
        fn read(
            &self,
            persist: &Persist,
            topic: &str,
            since: Option<&str>,
        ) -> Result<Vec<StoredMessage>, String> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let mut fail_topic = self.fail_topic.lock().unwrap();
            if fail_topic.as_deref() == Some(topic) {
                fail_topic.take();
                return Err(format!("injected read failure for {topic}"));
            }
            drop(fail_topic);
            PersistActionLaneReader.read(persist, topic, since)
        }
    }

    struct GateReplyWriter {
        allowed: Arc<AtomicBool>,
        attempts: Arc<AtomicUsize>,
    }

    impl ReplyWriter for GateReplyWriter {
        fn write(&self, persist: &Persist, request_ulid: &str, body: &str) -> Result<(), String> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if !self.allowed.load(Ordering::SeqCst) {
                return Err("injected reply publication failure".into());
            }
            PersistReplyWriter.write(persist, request_ulid, body)
        }
    }

    fn signed_sync_body(nonce: &str) -> String {
        authorize_test_body(
            AUTH_KEY,
            r#"{"schema_version":1,"request":"sync-now"}"#,
            MutationContext {
                verb: CUPS_SYNC_AUTH_VERB,
                node: "testpeer",
                target: "testpeer",
            },
            nonce,
            AUTH_NOW + 30_000,
        )
    }

    fn write_action(persist: &Persist, verb: &str, body: Option<&str>) -> StoredMessage {
        persist
            .write(&action_topic(verb), Priority::Default, None, body)
            .unwrap()
    }

    #[test]
    fn parse_lpstat_e_lists_queues() {
        assert_eq!(
            parse_lpstat_e("Office\nBackroom\n\n  Label  \n"),
            vec!["Office", "Backroom", "Label"]
        );
    }

    #[test]
    fn device_uri_parsed_and_classified() {
        let out =
            "device for Office: ipp://10.0.0.5:631/ipp/print\ndevice for USBHP: usb://HP/LaserJet";
        assert_eq!(
            parse_device_uri(out, "Office").as_deref(),
            Some("ipp://10.0.0.5:631/ipp/print")
        );
        assert_eq!(
            queue_kind(&parse_device_uri(out, "Office").unwrap()),
            QueueKind::Everywhere
        );
        assert_eq!(
            queue_kind(&parse_device_uri(out, "USBHP").unwrap()),
            QueueKind::Ppd
        );
    }

    #[test]
    fn own_record_shape() {
        let rec = own_record(
            "anvil",
            "10.42.0.7",
            &[("Office".to_string(), QueueKind::Everywhere)],
            123,
        );
        assert_eq!(rec["host"], "anvil");
        assert_eq!(rec["written_at_ms"], 123);
        assert_eq!(rec["queues"][0]["name"], "Office");
        assert_eq!(rec["queues"][0]["kind"], "everywhere");
        assert_eq!(
            rec["queues"][0]["ipp_path"],
            "ipp://10.42.0.7:631/printers/Office"
        );
    }

    #[test]
    fn import_plan_names_at_host_and_excludes_self() {
        let anvil = own_record(
            "anvil",
            "10.42.0.7",
            &[("Office".to_string(), QueueKind::Everywhere)],
            1,
        );
        let forge = own_record(
            "forge",
            "10.42.0.8",
            &[("Lab".to_string(), QueueKind::Ppd)],
            1,
        );
        let plan = import_plan("anvil", &[anvil, forge]);
        // Only forge's queue imported (self excluded).
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].local_name, "Lab@forge");
        assert_eq!(plan[0].uri, "ipp://10.42.0.8:631/printers/Lab");
        assert_eq!(plan[0].kind, QueueKind::Ppd);
    }

    #[test]
    fn lpadmin_argv_everywhere_vs_ppd() {
        let q = ImportQueue {
            local_name: "Lab@forge".into(),
            uri: "ipp://10.42.0.8:631/printers/Lab".into(),
            kind: QueueKind::Everywhere,
            host: "forge".into(),
            queue: "Lab".into(),
        };
        let argv = lpadmin_add_argv(&q, None);
        assert!(argv.windows(2).any(|w| w == ["-m", "everywhere"]));
        let q2 = ImportQueue {
            kind: QueueKind::Ppd,
            ..q
        };
        let argv2 = lpadmin_add_argv(&q2, Some("/mesh/ppd/forge/Lab.ppd"));
        assert!(argv2
            .windows(2)
            .any(|w| w == ["-P", "/mesh/ppd/forge/Lab.ppd"]));
    }

    #[test]
    fn prune_targets_only_stale_at_host_queues() {
        let existing = vec![
            "Office".to_string(),     // local — never pruned
            "Lab@forge".to_string(),  // desired — keep
            "Old@beacon".to_string(), // vanished — prune
        ];
        let desired = vec!["Lab@forge".to_string()];
        assert_eq!(
            prune_list(&existing, &desired),
            vec!["Old@beacon".to_string()]
        );
    }

    #[test]
    fn defaults_lww_highest_timestamp_wins() {
        let a = json!({"default_printer": "Office@anvil", "written_at_ms": 10});
        let b = json!({"default_printer": "Lab@forge", "written_at_ms": 20});
        let w = resolve_defaults_lww(&[a, b]).unwrap();
        assert_eq!(w["default_printer"], "Lab@forge");
    }

    #[test]
    fn cupsd_listen_idempotence_and_overlay_only() {
        let base = "LogLevel warn\nListen localhost:631\n";
        assert!(cupsd_needs_listen(base, "10.42.0.7"));
        let next = cupsd_with_listen(base, "10.42.0.7", "10.42.0.0/16");
        assert!(next.contains("Listen 10.42.0.7:631"));
        assert!(next.contains("Allow from 10.42.0.0/16"));
        assert!(!next.contains("Listen 0.0.0.0"));
        // Second pass: already present → no rewrite needed.
        assert!(!cupsd_needs_listen(&next, "10.42.0.7"));
    }

    // ── PRINT-8.b: handle_action pure dispatch ─────────────────────────────

    fn test_worker() -> CupsSyncWorker {
        CupsSyncWorker {
            tick: DEFAULT_TICK_INTERVAL,
            mesh_home: PathBuf::from("/nonexistent/mesh-home"),
            overlay_ip_path: PathBuf::from("/nonexistent/overlay-ip"),
            hostname: "testpeer".to_string(),
            overlay_cidr: "10.42.0.0/16".to_string(),
            lpstat: "lpstat".to_string(),
            lpadmin: "lpadmin".to_string(),
            lpoptions: "lpoptions".to_string(),
            cupsctl: "cupsctl".to_string(),
            bus_root_override: None,
            action_cursors: HashMap::new(),
            bus_factory: Arc::new(PersistCupsBusFactory),
            action_reader: Arc::new(PersistActionLaneReader),
            reply_writer: Arc::new(PersistReplyWriter),
            pending_replies: HashMap::new(),
            authorizer: Arc::new(ActionAuthorizer::production()),
            action_effect: None,
        }
    }

    #[test]
    fn handle_action_list_returns_json_array() {
        let w = test_worker();
        let reply = w.handle_action("list");
        // printers dir doesn't exist → empty array, not an error.
        let v: serde_json::Value = serde_json::from_str(&reply).expect("valid JSON");
        assert!(v.is_array(), "expected array, got: {reply}");
    }

    #[test]
    fn handle_action_sync_now_returns_ok() {
        let w = test_worker();
        // tick_once is a no-op when lpstat/lpadmin are absent or not installed.
        let reply = w.handle_action("sync-now");
        let v: serde_json::Value = serde_json::from_str(&reply).expect("valid JSON");
        assert_eq!(v["ok"], serde_json::json!(true), "got: {reply}");
    }

    #[test]
    fn handle_action_unknown_verb_returns_error() {
        let w = test_worker();
        let reply = w.handle_action("frobnicate");
        let v: serde_json::Value = serde_json::from_str(&reply).expect("valid JSON");
        assert!(v["error"].is_string(), "got: {reply}");
    }

    #[test]
    fn action_verbs_are_the_locked_two() {
        assert_eq!(ACTION_VERBS, ["sync-now", "list"]);
    }

    #[test]
    fn sync_now_refuses_unsigned_tampered_and_replayed_bodies() {
        let auth_root = tempfile::tempdir().expect("auth root");
        let w = test_worker().with_authorizer(std::sync::Arc::new(ActionAuthorizer::for_test(
            AUTH_KEY,
            auth_root.path().to_path_buf(),
            AUTH_NOW,
        )));
        let unsigned = r#"{"schema_version":1,"request":"sync-now"}"#;
        let context = MutationContext {
            verb: CUPS_SYNC_AUTH_VERB,
            node: "testpeer",
            target: "testpeer",
        };
        let armed = authorize_test_body(
            AUTH_KEY,
            unsigned,
            context,
            "cups-sync-once",
            AUTH_NOW + 30_000,
        );
        let tampered = armed.replace("sync-now", "sync-now-tampered");

        let unsigned_reply = w.handle_bus_action("sync-now", Some(unsigned));
        assert!(unsigned_reply.contains("authorization refused"));
        let tampered_reply = w.handle_bus_action("sync-now", Some(&tampered));
        assert!(
            tampered_reply.contains("authorization refused"),
            "tampered sync request must be refused: {tampered_reply}"
        );
        assert_eq!(
            w.handle_bus_action("sync-now", Some(&armed)),
            r#"{"ok":true}"#
        );
        let replay = w.handle_bus_action("sync-now", Some(&armed));
        assert!(replay.contains("already used"));
    }

    #[test]
    fn list_stays_read_only_and_does_not_require_a_capability() {
        let reply = test_worker().handle_bus_action("list", None);
        let value: serde_json::Value = serde_json::from_str(&reply).expect("valid JSON");
        assert!(value.is_array());
    }

    #[tokio::test]
    async fn late_bus_atomic_tail_prime_and_forward_sync_once() {
        let dir = tempfile::tempdir().unwrap();
        let bus_root = dir.path().join("bus");
        let persist = Persist::open(bus_root.clone()).unwrap();
        let retained_sync = write_action(&persist, "sync-now", Some(&signed_sync_body("retained")));
        let retained_list = write_action(&persist, "list", None);

        let attempts = Arc::new(AtomicUsize::new(0));
        let reader = Arc::new(HostileLaneReader::default());
        reader.fail_next(&action_topic("list"));
        let sync_effects = Arc::new(AtomicUsize::new(0));
        let list_effects = Arc::new(AtomicUsize::new(0));
        let sync_effects_for_action = Arc::clone(&sync_effects);
        let list_effects_for_action = Arc::clone(&list_effects);
        let auth_root = dir.path().join("auth");
        let mut worker = test_worker()
            .with_bus_root(bus_root.clone())
            .with_tick(Duration::from_millis(5))
            .with_bus_factory(Arc::new(LateBusFactory {
                root: bus_root,
                attempts: Arc::clone(&attempts),
            }))
            .with_action_reader(reader.clone())
            .with_authorizer(Arc::new(ActionAuthorizer::for_test(
                AUTH_KEY, auth_root, AUTH_NOW,
            )))
            .with_action_effect(Arc::new(move |verb| match verb {
                "sync-now" => {
                    sync_effects_for_action.fetch_add(1, Ordering::SeqCst);
                    r#"{"ok":true}"#.to_string()
                }
                "list" => {
                    list_effects_for_action.fetch_add(1, Ordering::SeqCst);
                    "[]".to_string()
                }
                _ => unreachable!(),
            }));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::timeout(Duration::from_secs(3), async {
            while reader.reads.load(Ordering::SeqCst) < 4 {
                assert!(!task.is_finished(), "worker exited during Bus activation");
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("atomic tail-prime recovery");
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(sync_effects.load(Ordering::SeqCst), 0);
        assert_eq!(list_effects.load(Ordering::SeqCst), 0);
        assert!(persist
            .list_since(&reply_topic(&retained_sync.ulid), None)
            .unwrap()
            .is_empty());
        assert!(persist
            .list_since(&reply_topic(&retained_list.ulid), None)
            .unwrap()
            .is_empty());

        let forward = write_action(
            &persist,
            "sync-now",
            Some(&signed_sync_body("forward-once")),
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            while persist
                .list_since(&reply_topic(&forward.ulid), None)
                .unwrap()
                .is_empty()
            {
                assert!(!task.is_finished(), "worker exited before forward sync");
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("forward sync reply");
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(sync_effects.load(Ordering::SeqCst), 1);
        assert_eq!(list_effects.load(Ordering::SeqCst), 0);
        assert!(attempts.load(Ordering::SeqCst) >= 4);

        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("shutdown timeout")
            .expect("worker join")
            .expect("worker result");
    }

    #[test]
    fn final_lane_read_and_reply_failure_are_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().join("bus")).unwrap();
        let reader = Arc::new(HostileLaneReader::default());
        let allowed = Arc::new(AtomicBool::new(false));
        let reply_attempts = Arc::new(AtomicUsize::new(0));
        let sync_effects = Arc::new(AtomicUsize::new(0));
        let sync_effects_for_action = Arc::clone(&sync_effects);
        let mut worker = test_worker()
            .with_action_reader(reader.clone())
            .with_reply_writer(Arc::new(GateReplyWriter {
                allowed: Arc::clone(&allowed),
                attempts: Arc::clone(&reply_attempts),
            }))
            .with_authorizer(Arc::new(ActionAuthorizer::for_test(
                AUTH_KEY,
                dir.path().join("auth"),
                AUTH_NOW,
            )))
            .with_action_effect(Arc::new(move |verb| {
                assert_eq!(verb, "sync-now");
                sync_effects_for_action.fetch_add(1, Ordering::SeqCst);
                r#"{"ok":true}"#.to_string()
            }));
        reader.fail_next(&action_topic("list"));
        assert!(worker.prime_action_cursors(&persist).is_err());
        assert!(
            worker.action_cursors.is_empty(),
            "failed final-lane tail read must install no partial cursor"
        );
        worker.action_cursors = worker.prime_action_cursors(&persist).unwrap();
        let cursors_before = worker.action_cursors.clone();
        let request = write_action(
            &persist,
            "sync-now",
            Some(&signed_sync_body("hostile-runtime")),
        );

        reader.fail_next(&action_topic("list"));
        assert!(worker.stage_actions(&persist).is_err());
        assert_eq!(worker.action_cursors, cursors_before);
        assert_eq!(sync_effects.load(Ordering::SeqCst), 0);
        assert!(worker.pending_replies.is_empty());

        let staged = worker.stage_actions(&persist).unwrap();
        assert!(worker.process_staged_actions(&persist, staged).is_err());
        assert_eq!(sync_effects.load(Ordering::SeqCst), 1);
        assert_eq!(worker.action_cursors, cursors_before);
        assert!(worker.pending_replies.contains_key(&request.ulid));
        assert!(persist
            .list_since(&reply_topic(&request.ulid), None)
            .unwrap()
            .is_empty());

        allowed.store(true, Ordering::SeqCst);
        let staged = worker.stage_actions(&persist).unwrap();
        worker.process_staged_actions(&persist, staged).unwrap();
        assert_eq!(sync_effects.load(Ordering::SeqCst), 1);
        assert!(worker.pending_replies.is_empty());
        assert_eq!(
            worker.action_cursors.get(&action_topic("sync-now")),
            Some(&request.ulid)
        );
        assert_eq!(
            persist
                .list_since(&reply_topic(&request.ulid), None)
                .unwrap()
                .len(),
            1
        );
        assert!(reply_attempts.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn dynamic_first_command_system_fallback_and_retry_shutdown() {
        assert_eq!(
            cups_sync_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        let explicit = PathBuf::from("/tmp/cups-sync-explicit-bus");
        assert_eq!(
            cups_sync_bus_root_or_system(Some(explicit.clone())),
            explicit
        );

        let dir = tempfile::tempdir().unwrap();
        let bus_root = dir.path().join("bus");
        let persist = Persist::open(bus_root.clone()).unwrap();
        let reader = Arc::new(HostileLaneReader::default());
        let list_effects = Arc::new(AtomicUsize::new(0));
        let list_effects_for_action = Arc::clone(&list_effects);
        let mut worker = test_worker()
            .with_bus_root(bus_root)
            .with_tick(Duration::from_millis(5))
            .with_action_reader(reader.clone())
            .with_action_effect(Arc::new(move |verb| {
                assert_eq!(verb, "list");
                list_effects_for_action.fetch_add(1, Ordering::SeqCst);
                "[]".to_string()
            }));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );
        tokio::time::timeout(Duration::from_secs(2), async {
            while reader.reads.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("empty-lane activation");
        tokio::time::sleep(Duration::from_millis(5)).await;
        let first = write_action(&persist, "list", None);
        tokio::time::timeout(Duration::from_secs(2), async {
            while persist
                .list_since(&reply_topic(&first.ulid), None)
                .unwrap()
                .is_empty()
            {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("first dynamically created list command");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(list_effects.load(Ordering::SeqCst), 1);
        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("dynamic worker shutdown")
            .expect("worker join")
            .expect("worker result");

        let attempts = Arc::new(AtomicUsize::new(0));
        let mut unavailable = test_worker()
            .with_tick(Duration::from_millis(5))
            .with_bus_factory(Arc::new(UnavailableBusFactory {
                attempts: Arc::clone(&attempts),
            }));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move {
            unavailable
                .run(ShutdownToken::from_receiver(shutdown_rx))
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while attempts.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("Bus retry attempts");
        assert!(!task.is_finished());
        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("retry shutdown timeout")
            .expect("worker join")
            .expect("worker result");
    }
}
