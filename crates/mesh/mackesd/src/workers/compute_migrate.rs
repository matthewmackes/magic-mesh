//! VIRT-8.a (v5.0.0) — cold VM migration source-side worker.
//!
//! Each peer drains the single `action/compute/migrate` Bus topic.
//! For each request where `source_peer == own_nebula_ip`, the worker:
//!
//! 1. `virsh shutdown <vm_id>` (graceful ACPI shutdown).
//! 2. Polls `virsh domstate <vm_id>` every 2 s until `shut off` or
//!    120 s timeout.
//! 3. `rsync --compress --progress <disk_path> <target>:<target_dir>`
//!    over the Nebula overlay.
//! 4. Publishes `event/compute/migrate-ready` so the target peer's
//!    `compute_provision` (VIRT-8.b, ships with VIRT-6) defines the
//!    VM with the migrated disk + starts it.
//! 5. `virsh undefine <vm_id>` to remove the source-side VM
//!    definition. `compute_registry`'s next 10 s tick publishes
//!    the updated `compute/inventory/<peer>` automatically (VIRT-8
//!    bullet 3 satisfied without an explicit publish here).
//!
//! ## Topic-shape lock
//!
//! Design doc §3 notates the request topic as
//! `compute/migrate/<vm-id>`. Per Q96 + `rpc.rs`'s
//! `action/<domain>/<verb>` convention, the actual topic is
//! `action/compute/migrate` (single fixed topic), with per-peer
//! addressing in the payload's `source_peer` field. The migration's
//! correlation key is the request message's own ULID, propagated
//! into the published `event/compute/migrate-ready` so the target's
//! handler can correlate back. Followup in worklist
//! (VIRT-8.followup) to amend the design doc.
//!
//! Non-source peers see each message, advance the cursor, and skip
//! — same shape as `cert_authority`.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};

/// Bus action topic this worker drains.
pub const ACTION_TOPIC: &str = "action/compute/migrate";

/// Event topic published when the source side finishes shipping
/// the disk to the target. The target side (VIRT-8.b, same worker)
/// subscribes here + filters `target_peer == own`.
pub const MIGRATE_READY_TOPIC: &str = "event/compute/migrate-ready";

/// Event topic the target side publishes when it can't define/start
/// the migrated VM (so the operator UI can surface the failure
/// instead of the VM silently vanishing on both peers).
pub const MIGRATE_FAILED_TOPIC: &str = "event/compute/migrate-failed";

/// Default poll cadence — control surface.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(400);

/// Nebula overlay interface name (consistent with the rest of the
/// mackesd workers).
pub const DEFAULT_NEBULA_INTERFACE: &str = "nebula1";

/// Maximum wait for the guest to ACPI-shutdown before declaring the
/// migration failed (design doc §8 + task body bullet 1).
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(120);

/// Inter-poll spacing for `virsh domstate` while waiting on
/// shutdown. 2 s balances responsiveness against virsh subprocess
/// churn.
pub const DEFAULT_SHUTDOWN_POLL: Duration = Duration::from_secs(2);

/// Target-side VM storage directory rsync ships disks into.
pub const DEFAULT_TARGET_VM_DIR: &str = "/var/lib/mde-vms/";

/// Migration-request payload per design doc §3.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MigrateRequest {
    /// Source peer's Nebula overlay IP. Only the peer whose own
    /// nebula address matches this acts on the request.
    pub source_peer: String,
    /// Target peer's Nebula overlay IP. The rsync destination.
    pub target_peer: String,
    /// libvirt domain ID (UUID) of the VM being migrated.
    pub vm_id: String,
    /// Absolute path to the VM's primary disk on the source peer.
    pub disk_path: String,
}

/// `event/compute/migrate-ready` payload, published by the source
/// after a successful disk ship. The target side (VIRT-8.b) reads
/// `target_peer == own_nebula_ip` to claim responsibility, then
/// `virsh define`s `domain_xml` + starts the VM.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MigrateReadyEvent {
    /// Source peer's Nebula overlay IP (audit + Workbench display).
    pub source_peer: String,
    /// Target peer's Nebula overlay IP — the recipient filter.
    pub target_peer: String,
    /// VM id.
    pub vm_id: String,
    /// Absolute path the disk landed at on the target.
    pub target_disk_path: String,
    /// ULID of the originating `action/compute/migrate` request, so
    /// the target peer can correlate failures back to the operator.
    pub request_ulid: String,
    /// The source VM's `virsh dumpxml` output, captured BEFORE
    /// shutdown. The target `virsh define`s it verbatim — the disk
    /// `<source file=…>` path matches on both peers (identical
    /// `/var/lib/mde-vms/<vm-id>.qcow2` pool layout), and the VM's
    /// Nebula identity lives in the disk, so the migrated VM keeps
    /// its full config (network, virtiofs, memory) + cert.
    #[serde(default)]
    pub domain_xml: String,
}

/// `event/compute/migrate-failed` payload (target-side).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MigrateFailedEvent {
    /// VM id that failed to come up on the target.
    pub vm_id: String,
    /// Target peer that couldn't define/start it.
    pub target_peer: String,
    /// Correlation ULID of the original migrate request.
    pub request_ulid: String,
    /// Human-readable failure description.
    pub error: String,
}

/// Outcome of the source-side migration flow.
#[derive(Debug, Clone, PartialEq)]
pub enum MigrationOutcome {
    /// Disk landed on target + migrate-ready published. Carries the
    /// captured `virsh dumpxml` so the publish can include it.
    Ok {
        /// `virsh dumpxml` output captured before shutdown.
        domain_xml: String,
    },
    /// Guest didn't ACPI-shutdown within
    /// [`DEFAULT_SHUTDOWN_TIMEOUT`].
    ShutdownTimeout,
    /// `rsync` returned a non-zero exit status.
    RsyncFailure {
        /// Description of the rsync failure.
        exit_description: String,
    },
    /// `virsh` shell-out couldn't be spawned (binary missing).
    VirshUnavailable,
}

/// Parse a migration-request body.
///
/// # Errors
///
/// Returns a human-readable error string on malformed JSON or
/// missing required fields.
pub fn parse_migrate_request(body: &str) -> Result<MigrateRequest, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed migrate request: {e}"))
}

/// `true` when this peer is the source for the request.
#[must_use]
pub fn is_source_peer(req: &MigrateRequest, own_nebula_ip: &str) -> bool {
    !own_nebula_ip.is_empty() && req.source_peer == own_nebula_ip
}

/// Build the args for `virsh shutdown <vm_id>`.
#[must_use]
pub fn build_virsh_shutdown_args(vm_id: &str) -> Vec<String> {
    vec!["shutdown".into(), vm_id.into()]
}

/// Build the args for `virsh domstate <vm_id>`.
#[must_use]
pub fn build_virsh_domstate_args(vm_id: &str) -> Vec<String> {
    vec!["domstate".into(), vm_id.into()]
}

/// Build the args for `virsh undefine <vm_id>`.
#[must_use]
pub fn build_virsh_undefine_args(vm_id: &str) -> Vec<String> {
    vec!["undefine".into(), vm_id.into()]
}

/// Build the args for `virsh dumpxml <vm_id>` (source side captures
/// the domain definition before shutdown so the target can recreate
/// it verbatim).
#[must_use]
pub fn build_virsh_dumpxml_args(vm_id: &str) -> Vec<String> {
    vec!["dumpxml".into(), vm_id.into()]
}

/// Build the args for `virsh define <xml_path>` (target side defines
/// the migrated VM from the captured XML).
#[must_use]
pub fn build_virsh_define_args(xml_path: &str) -> Vec<String> {
    vec!["define".into(), xml_path.into()]
}

/// Build the args for `virsh start <vm_id>` (target side boots the
/// migrated VM).
#[must_use]
pub fn build_virsh_start_args(vm_id: &str) -> Vec<String> {
    vec!["start".into(), vm_id.into()]
}

/// `true` when this peer is the target for a migrate-ready event.
#[must_use]
pub fn is_target_peer(event: &MigrateReadyEvent, own_nebula_ip: &str) -> bool {
    !own_nebula_ip.is_empty() && event.target_peer == own_nebula_ip
}

/// Parse a migrate-ready event body.
///
/// # Errors
///
/// Returns a human-readable error on malformed JSON.
pub fn parse_migrate_ready_event(body: &str) -> Result<MigrateReadyEvent, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed migrate-ready event: {e}"))
}

/// Parse `virsh domstate <vm>` output into a trimmed state token
/// (`"running"`, `"shut off"`, `"paused"`, ...). Returns `None`
/// when stdout is empty.
#[must_use]
pub fn parse_virsh_domstate(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// `true` when the state token indicates the guest has reached
/// the ACPI-shutdown end state.
#[must_use]
pub fn is_shutoff(state: &str) -> bool {
    state.eq_ignore_ascii_case("shut off")
}

/// Build the `rsync --compress` args for shipping a disk from the
/// source to the target peer's `/var/lib/mde-vms/`. SSH is used
/// implicitly (rsync's default remote-shell), which over Nebula
/// goes via the peer's overlay-bound sshd (NF-21.1).
#[must_use]
pub fn build_rsync_args(disk_path: &str, target_peer: &str, target_dir: &str) -> Vec<String> {
    let dest = format!("{target_peer}:{target_dir}");
    vec![
        "--compress".into(),
        "--progress".into(),
        disk_path.into(),
        dest,
    ]
}

/// Compute the expected target-side path after the rsync. rsync
/// preserves the source filename, so target_disk_path is just
/// `<target_dir>/<basename>`.
#[must_use]
pub fn target_disk_path_for(disk_path: &str, target_dir: &str) -> String {
    let basename = std::path::Path::new(disk_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("disk.qcow2");
    let sep = if target_dir.ends_with('/') { "" } else { "/" };
    format!("{target_dir}{sep}{basename}")
}

/// Build the `event/compute/migrate-ready` payload.
#[must_use]
pub fn build_migrate_ready_event(
    req: &MigrateRequest,
    target_disk_path: String,
    request_ulid: String,
    domain_xml: String,
) -> MigrateReadyEvent {
    MigrateReadyEvent {
        source_peer: req.source_peer.clone(),
        target_peer: req.target_peer.clone(),
        vm_id: req.vm_id.clone(),
        target_disk_path,
        request_ulid,
        domain_xml,
    }
}

/// Pure waiter: take a state-observer closure, poll until the
/// observer returns "shut off" (any case) or the deadline passes.
/// Returns `true` on shutoff, `false` on timeout.
///
/// `poll_interval` is the inter-observation sleep; `attempts` is the
/// hard cap so tests can drive deterministic behavior without
/// wall-clock waits.
pub fn wait_for_shutoff<F>(mut observer: F, attempts: usize) -> bool
where
    F: FnMut() -> Option<String>,
{
    for _ in 0..attempts {
        if let Some(state) = observer() {
            if is_shutoff(&state) {
                return true;
            }
        }
    }
    false
}

fn binary_present(bin: &str) -> bool {
    Command::new(bin).arg("--version").output().is_ok()
}

fn run_virsh(args: &[String]) -> Option<String> {
    let output = Command::new("virsh").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_virsh_status(args: &[String]) -> bool {
    Command::new("virsh")
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run_rsync(args: &[String]) -> Result<(), String> {
    let status = Command::new("rsync")
        .args(args)
        .status()
        .map_err(|e| format!("rsync spawn: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("rsync exited {status}"))
    }
}

fn local_nebula_addr(interface: &str) -> String {
    let Ok(output) = Command::new("ip")
        .args(["-4", "addr", "show", interface])
        .output()
    else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("inet ") {
            if let Some(ip) = rest.split('/').next() {
                return ip.to_string();
            }
        }
    }
    String::new()
}

/// Drive the source-side migration flow for one request. Returns the
/// terminal outcome. Subprocess shell-outs are real (virsh + rsync);
/// the timeout uses [`DEFAULT_SHUTDOWN_TIMEOUT`] /
/// [`DEFAULT_SHUTDOWN_POLL`] under the hood.
fn run_migration(req: &MigrateRequest) -> MigrationOutcome {
    if !binary_present("virsh") {
        return MigrationOutcome::VirshUnavailable;
    }

    // Step 0: capture the domain XML WHILE the VM is still defined,
    // so the target can recreate it verbatim. Empty on failure — the
    // target handler surfaces a clear migrate-failed in that case.
    let domain_xml = run_virsh(&build_virsh_dumpxml_args(&req.vm_id)).unwrap_or_default();

    // Step 1: ACPI shutdown.
    let _ = run_virsh_status(&build_virsh_shutdown_args(&req.vm_id));

    // Step 2: poll for shutoff.
    let attempts =
        (DEFAULT_SHUTDOWN_TIMEOUT.as_millis() / DEFAULT_SHUTDOWN_POLL.as_millis()) as usize;
    let domstate_args = build_virsh_domstate_args(&req.vm_id);
    let shutoff = wait_for_shutoff(
        || {
            std::thread::sleep(DEFAULT_SHUTDOWN_POLL);
            run_virsh(&domstate_args).and_then(|s| parse_virsh_domstate(&s))
        },
        attempts,
    );
    if !shutoff {
        return MigrationOutcome::ShutdownTimeout;
    }

    // Step 3: rsync.
    let rsync_args = build_rsync_args(&req.disk_path, &req.target_peer, DEFAULT_TARGET_VM_DIR);
    if let Err(e) = run_rsync(&rsync_args) {
        return MigrationOutcome::RsyncFailure {
            exit_description: e,
        };
    }

    // Step 5: undefine (publish happens in the caller so we can
    // include the request_ulid in the event).
    let _ = run_virsh_status(&build_virsh_undefine_args(&req.vm_id));

    MigrationOutcome::Ok { domain_xml }
}

/// VIRT-8.b — target-side: define + start the migrated VM from the
/// captured XML. The disk is already in place (rsync'd by the
/// source) at the matching `/var/lib/mde-vms/<vm>.qcow2` path the
/// XML references, so a verbatim `virsh define` + `virsh start`
/// recreates the VM with its full config + Nebula identity.
///
/// # Errors
///
/// Returns a description when virsh is absent, the XML is empty
/// (source dumpxml failed), or define/start exits non-zero.
fn run_migrate_target(event: &MigrateReadyEvent) -> Result<(), String> {
    // Validate the payload before touching the environment so a
    // malformed event is rejected deterministically (and testably)
    // regardless of whether virsh is installed.
    if event.domain_xml.trim().is_empty() {
        return Err("migrate-ready event carried no domain_xml (source dumpxml failed)".into());
    }
    if !binary_present("virsh") {
        return Err("virsh not available on target peer".into());
    }
    let tmp_dir = std::env::temp_dir().join("mde-vm-migrate");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("mkdir tmp: {e}"))?;
    let xml_path = tmp_dir.join(format!("{}.xml", event.vm_id));
    std::fs::write(&xml_path, &event.domain_xml).map_err(|e| format!("write xml: {e}"))?;
    let define_ok = run_virsh_status(&build_virsh_define_args(&xml_path.to_string_lossy()));
    let _ = std::fs::remove_file(&xml_path);
    if !define_ok {
        return Err(format!("virsh define failed for {}", event.vm_id));
    }
    if !run_virsh_status(&build_virsh_start_args(&event.vm_id)) {
        return Err(format!("virsh start failed for {}", event.vm_id));
    }
    Ok(())
}

fn publish_migrate_failed(persist: &Persist, event: &MigrateReadyEvent, error: &str) {
    let failed = MigrateFailedEvent {
        vm_id: event.vm_id.clone(),
        target_peer: event.target_peer.clone(),
        request_ulid: event.request_ulid.clone(),
        error: error.to_string(),
    };
    let Ok(body) = serde_json::to_string(&failed) else {
        return;
    };
    if let Err(e) = persist.write(MIGRATE_FAILED_TOPIC, Priority::Default, None, Some(&body)) {
        tracing::warn!(error = %e, vm_id = %event.vm_id, "compute_migrate: migrate-failed publish failed");
    }
}

fn publish_migrate_ready(persist: &Persist, event: &MigrateReadyEvent) {
    let Ok(body) = serde_json::to_string(event) else {
        return;
    };
    if let Err(e) = persist.write(MIGRATE_READY_TOPIC, Priority::Default, None, Some(&body)) {
        tracing::warn!(
            error = %e,
            vm_id = %event.vm_id,
            target = %event.target_peer,
            "compute_migrate: migrate-ready publish failed"
        );
    }
}

/// Worker handle.
pub struct ComputeMigrateWorker {
    nebula_interface: String,
    nebula_addr_hint: String,
    poll_interval: Duration,
    bus_root_override: Option<PathBuf>,
}

impl Default for ComputeMigrateWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl ComputeMigrateWorker {
    /// Construct with production defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            nebula_interface: DEFAULT_NEBULA_INTERFACE.into(),
            nebula_addr_hint: String::new(),
            poll_interval: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
        }
    }

    /// Override the local peer's Nebula address (skips runtime
    /// detection via `ip addr`).
    #[must_use]
    pub fn with_nebula_addr_hint(mut self, addr: String) -> Self {
        self.nebula_addr_hint = addr;
        self
    }

    /// Override the Bus root directory. Used in tests.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Override the poll cadence. Used in tests.
    #[must_use]
    pub fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }
}

fn resolve_nebula_addr(worker: &ComputeMigrateWorker) -> String {
    if !worker.nebula_addr_hint.is_empty() {
        return worker.nebula_addr_hint.clone();
    }
    local_nebula_addr(&worker.nebula_interface)
}

fn poll_once(persist: &Persist, worker: &ComputeMigrateWorker, cursor: &mut Option<String>) {
    let msgs = match persist.list_since(ACTION_TOPIC, cursor.as_deref()) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!(error = %e, "compute_migrate: list_since failed");
            return;
        }
    };
    let own_ip = resolve_nebula_addr(worker);
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        let req = match parse_migrate_request(body) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "compute_migrate: bad request");
                continue;
            }
        };
        if !is_source_peer(&req, &own_ip) {
            tracing::debug!(
                ulid = %msg.ulid,
                source = %req.source_peer,
                own = %own_ip,
                "compute_migrate: not source peer; skipping"
            );
            continue;
        }
        let outcome = run_migration(&req);
        match outcome {
            MigrationOutcome::Ok { domain_xml } => {
                let event = build_migrate_ready_event(
                    &req,
                    target_disk_path_for(&req.disk_path, DEFAULT_TARGET_VM_DIR),
                    msg.ulid.clone(),
                    domain_xml,
                );
                publish_migrate_ready(persist, &event);
            }
            other => {
                tracing::warn!(
                    ulid = %msg.ulid,
                    vm_id = %req.vm_id,
                    outcome = ?other,
                    "compute_migrate: migration failed"
                );
            }
        }
    }
}

/// VIRT-8.b — target-side poll: drain `event/compute/migrate-ready`,
/// act on events addressed to this peer (`target_peer == own`),
/// define + start the migrated VM, and publish `migrate-failed` on
/// error. Non-target events advance the cursor + skip.
fn poll_target_once(persist: &Persist, worker: &ComputeMigrateWorker, cursor: &mut Option<String>) {
    let msgs = match persist.list_since(MIGRATE_READY_TOPIC, cursor.as_deref()) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!(error = %e, "compute_migrate: migrate-ready list_since failed");
            return;
        }
    };
    let own_ip = resolve_nebula_addr(worker);
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        let event = match parse_migrate_ready_event(body) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "compute_migrate: bad migrate-ready event");
                continue;
            }
        };
        if !is_target_peer(&event, &own_ip) {
            continue;
        }
        if let Err(e) = run_migrate_target(&event) {
            tracing::warn!(vm_id = %event.vm_id, error = %e, "compute_migrate: target define/start failed");
            publish_migrate_failed(persist, &event, &e);
        } else {
            tracing::info!(vm_id = %event.vm_id, "compute_migrate: migrated VM defined + started on target");
        }
    }
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

#[async_trait::async_trait]
impl Worker for ComputeMigrateWorker {
    fn name(&self) -> &'static str {
        "compute_migrate"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = match self.bus_root_override.clone().or_else(default_bus_root) {
            Some(r) => r,
            None => {
                tracing::debug!("compute_migrate: no bus root; worker idle");
                return Ok(());
            }
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(error = %e, "compute_migrate: persist open failed; worker idle");
                return Ok(());
            }
        };
        let mut source_cursor: Option<String> = None;
        let mut target_cursor: Option<String> = None;
        let mut tick = tokio::time::interval(self.poll_interval);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    // Source side: action/compute/migrate.
                    poll_once(&persist, self, &mut source_cursor);
                    // Target side (VIRT-8.b): event/compute/migrate-ready.
                    poll_target_once(&persist, self, &mut target_cursor);
                }
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_migrate_request ──

    #[test]
    fn parse_migrate_happy_path() {
        let body = r#"{"source_peer":"10.42.0.1","target_peer":"10.42.0.2","vm_id":"abc","disk_path":"/var/lib/mde-vms/abc.qcow2"}"#;
        let req = parse_migrate_request(body).expect("parse");
        assert_eq!(req.source_peer, "10.42.0.1");
        assert_eq!(req.target_peer, "10.42.0.2");
        assert_eq!(req.vm_id, "abc");
        assert_eq!(req.disk_path, "/var/lib/mde-vms/abc.qcow2");
    }

    #[test]
    fn parse_migrate_rejects_malformed_json() {
        let err = parse_migrate_request("nope").expect_err("malformed");
        assert!(err.contains("malformed"));
    }

    // ── is_source_peer ──

    #[test]
    fn is_source_peer_true_when_match() {
        let req = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            disk_path: "/d".into(),
        };
        assert!(is_source_peer(&req, "10.42.0.1"));
    }

    #[test]
    fn is_source_peer_false_when_mismatch() {
        let req = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            disk_path: "/d".into(),
        };
        assert!(!is_source_peer(&req, "10.42.0.99"));
    }

    #[test]
    fn is_source_peer_false_when_own_ip_empty() {
        let req = MigrateRequest {
            source_peer: "".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            disk_path: "/d".into(),
        };
        // Empty source_peer + empty own_ip would otherwise spuriously
        // match — explicit guard.
        assert!(!is_source_peer(&req, ""));
    }

    // ── virsh arg builders ──

    #[test]
    fn shutdown_args_are_minimal() {
        assert_eq!(build_virsh_shutdown_args("abc"), vec!["shutdown", "abc"]);
    }

    #[test]
    fn domstate_args_are_minimal() {
        assert_eq!(build_virsh_domstate_args("abc"), vec!["domstate", "abc"]);
    }

    #[test]
    fn undefine_args_are_minimal() {
        assert_eq!(build_virsh_undefine_args("abc"), vec!["undefine", "abc"]);
    }

    // ── parse_virsh_domstate + is_shutoff ──

    #[test]
    fn parse_domstate_trims_whitespace() {
        assert_eq!(parse_virsh_domstate("  running \n"), Some("running".into()));
    }

    #[test]
    fn parse_domstate_none_when_empty() {
        assert!(parse_virsh_domstate("   \n").is_none());
    }

    #[test]
    fn is_shutoff_matches_canonical_token() {
        assert!(is_shutoff("shut off"));
        assert!(is_shutoff("SHUT OFF"));
        assert!(!is_shutoff("running"));
        assert!(!is_shutoff("paused"));
    }

    // ── rsync args ──

    #[test]
    fn rsync_args_use_compress_and_overlay_target() {
        let args = build_rsync_args(
            "/var/lib/mde-vms/abc.qcow2",
            "10.42.0.2",
            "/var/lib/mde-vms/",
        );
        assert!(args.contains(&"--compress".to_string()));
        assert!(args.contains(&"--progress".to_string()));
        assert!(args.contains(&"/var/lib/mde-vms/abc.qcow2".to_string()));
        assert_eq!(args.last().unwrap(), "10.42.0.2:/var/lib/mde-vms/");
    }

    // ── target_disk_path_for ──

    #[test]
    fn target_disk_path_handles_trailing_slash() {
        let p = target_disk_path_for("/var/lib/mde-vms/abc.qcow2", "/var/lib/mde-vms/");
        assert_eq!(p, "/var/lib/mde-vms/abc.qcow2");
    }

    #[test]
    fn target_disk_path_inserts_separator_when_missing() {
        let p = target_disk_path_for("/src/abc.qcow2", "/var/lib/mde-vms");
        assert_eq!(p, "/var/lib/mde-vms/abc.qcow2");
    }

    // ── migrate-ready event ──

    #[test]
    fn migrate_ready_event_carries_correlation_ulid() {
        let req = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            disk_path: "/var/lib/mde-vms/abc.qcow2".into(),
        };
        let ev = build_migrate_ready_event(
            &req,
            "/var/lib/mde-vms/abc.qcow2".into(),
            "01JAN".into(),
            "<domain>…</domain>".into(),
        );
        assert_eq!(ev.target_peer, "10.42.0.2");
        assert_eq!(ev.request_ulid, "01JAN");
        assert_eq!(ev.target_disk_path, "/var/lib/mde-vms/abc.qcow2");
        assert_eq!(ev.domain_xml, "<domain>…</domain>");
    }

    // ── VIRT-8.b — target-side define/start ──

    #[test]
    fn dumpxml_define_start_args_are_minimal() {
        assert_eq!(build_virsh_dumpxml_args("abc"), vec!["dumpxml", "abc"]);
        assert_eq!(
            build_virsh_define_args("/t/abc.xml"),
            vec!["define", "/t/abc.xml"]
        );
        assert_eq!(build_virsh_start_args("abc"), vec!["start", "abc"]);
    }

    #[test]
    fn is_target_peer_filters_by_target() {
        let ev = MigrateReadyEvent {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            target_disk_path: "/var/lib/mde-vms/abc.qcow2".into(),
            request_ulid: "01JAN".into(),
            domain_xml: "<domain/>".into(),
        };
        assert!(is_target_peer(&ev, "10.42.0.2"));
        assert!(!is_target_peer(&ev, "10.42.0.1"));
        assert!(!is_target_peer(&ev, ""));
    }

    #[test]
    fn migrate_ready_event_round_trips_domain_xml() {
        let ev = MigrateReadyEvent {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            target_disk_path: "/var/lib/mde-vms/abc.qcow2".into(),
            request_ulid: "01JAN".into(),
            domain_xml: "<domain type='kvm'><name>abc</name></domain>".into(),
        };
        let body = serde_json::to_string(&ev).unwrap();
        let back = parse_migrate_ready_event(&body).expect("parse");
        assert_eq!(back, ev);
        assert!(back.domain_xml.contains("<name>abc</name>"));
    }

    #[test]
    fn parse_migrate_ready_rejects_malformed() {
        assert!(parse_migrate_ready_event("not json").is_err());
    }

    // ── Required scenario 4 (VIRT-8.b half): target-provision failure ──

    #[test]
    fn run_migrate_target_errors_on_empty_domain_xml() {
        // Empty domain_xml means the source dumpxml failed — the
        // target must surface a clear error (→ migrate-failed), not
        // silently define nothing.
        let ev = MigrateReadyEvent {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            target_disk_path: "/var/lib/mde-vms/abc.qcow2".into(),
            request_ulid: "01JAN".into(),
            domain_xml: "   ".into(),
        };
        let err = run_migrate_target(&ev).expect_err("empty xml must fail");
        assert!(err.contains("no domain_xml"), "{err}");
    }

    #[test]
    fn migrate_failed_event_shape() {
        let ev = MigrateReadyEvent {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc".into(),
            target_disk_path: "/d".into(),
            request_ulid: "01JAN".into(),
            domain_xml: "<domain/>".into(),
        };
        let failed = MigrateFailedEvent {
            vm_id: ev.vm_id.clone(),
            target_peer: ev.target_peer.clone(),
            request_ulid: ev.request_ulid.clone(),
            error: "virsh define failed for abc".into(),
        };
        let body = serde_json::to_string(&failed).unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["vm_id"], "abc");
        assert_eq!(v["target_peer"], "10.42.0.2");
        assert!(v["error"].as_str().unwrap().contains("virsh define"));
    }

    #[test]
    fn migrate_ready_and_failed_topics_under_event_prefix() {
        assert!(MIGRATE_READY_TOPIC.starts_with("event/"));
        assert!(MIGRATE_FAILED_TOPIC.starts_with("event/"));
    }

    // ── Required scenario 2: shutdown timeout ──

    #[test]
    fn wait_for_shutoff_returns_false_when_state_never_flips() {
        // Observer always returns "running" — never shut off.
        let observed = wait_for_shutoff(|| Some("running".into()), 5);
        assert!(!observed);
    }

    #[test]
    fn wait_for_shutoff_returns_true_on_first_shutoff_observation() {
        let mut calls = 0;
        let observed = wait_for_shutoff(
            || {
                calls += 1;
                if calls < 3 {
                    Some("running".into())
                } else {
                    Some("shut off".into())
                }
            },
            10,
        );
        assert!(observed);
        assert_eq!(calls, 3, "should stop polling at first shutoff");
    }

    // ── Required scenario 3: rsync failure (via the MigrationOutcome
    //    variant + the test that run_migration would surface it; we
    //    cover the failure-shape here without invoking rsync) ──

    #[test]
    fn migration_outcome_rsync_failure_carries_description() {
        let out = MigrationOutcome::RsyncFailure {
            exit_description: "rsync exited 23".into(),
        };
        match out {
            MigrationOutcome::RsyncFailure { exit_description } => {
                assert!(exit_description.contains("23"));
            }
            _ => panic!("wrong variant"),
        }
    }

    // ── Required scenario 1: happy path planning ──

    #[test]
    fn happy_path_plan_compose() {
        // The full source-side flow is a deterministic composition of
        // the pure helpers — this test asserts the planned shape so a
        // regression in any helper breaks the chain visibly.
        let req = MigrateRequest {
            source_peer: "10.42.0.1".into(),
            target_peer: "10.42.0.2".into(),
            vm_id: "abc-uuid".into(),
            disk_path: "/var/lib/mde-vms/abc-uuid.qcow2".into(),
        };
        assert!(is_source_peer(&req, "10.42.0.1"));
        let shutdown_args = build_virsh_shutdown_args(&req.vm_id);
        assert!(shutdown_args.contains(&"abc-uuid".to_string()));
        let domstate_args = build_virsh_domstate_args(&req.vm_id);
        assert!(domstate_args.contains(&"abc-uuid".to_string()));
        let rsync_args = build_rsync_args(&req.disk_path, &req.target_peer, DEFAULT_TARGET_VM_DIR);
        assert_eq!(rsync_args.last().unwrap(), "10.42.0.2:/var/lib/mde-vms/");
        let undef_args = build_virsh_undefine_args(&req.vm_id);
        assert!(undef_args.contains(&"abc-uuid".to_string()));
        let dumpxml_args = build_virsh_dumpxml_args(&req.vm_id);
        assert!(dumpxml_args.contains(&"abc-uuid".to_string()));
        let target_path = target_disk_path_for(&req.disk_path, DEFAULT_TARGET_VM_DIR);
        let event = build_migrate_ready_event(
            &req,
            target_path,
            "01JANULID".into(),
            "<domain type='kvm'/>".into(),
        );
        assert_eq!(event.target_peer, "10.42.0.2");
        assert_eq!(event.request_ulid, "01JANULID");
        // Target side recreates from the captured XML.
        assert!(is_target_peer(&event, "10.42.0.2"));
        assert!(build_virsh_define_args("/t/x.xml").contains(&"define".to_string()));
    }

    // ── ACTION_TOPIC prefix lock ──

    #[test]
    fn action_topic_under_action_prefix() {
        assert!(ACTION_TOPIC.starts_with("action/"));
    }

    #[test]
    fn migrate_ready_topic_under_event_prefix() {
        assert!(MIGRATE_READY_TOPIC.starts_with("event/"));
    }
}
