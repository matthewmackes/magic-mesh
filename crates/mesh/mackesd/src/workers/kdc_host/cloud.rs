//! KDC-MESH-8 — placement-local cloud inventory run-commands for the KDC host.
//!
//! Split out of the parent `kdc_host` god-file (behavior-preserving
//! relocation): the phone-triggered [`CloudCommand`] set that reads the
//! placement-local cloud inventory over public `action/cloud/*` read verbs.
//! Lifecycle is intentionally absent: stock KDE Connect commands carry no
//! bounded Workload identity or generation and therefore cannot safely publish
//! `action/workload/operation`.

use super::*;
use mackes_mesh_types::cloud::{
    decode_cloud_arm_credential, CloudArmSigner, CLOUD_ACTION_SCHEMA_VERSION, CLOUD_ARM_CREDENTIAL,
};

/// How long a phone-triggered cloud Bus round-trip waits for the provider
/// adapter's reply before honest-gating "cloud unavailable" (no fabricated
/// result).
const CLOUD_BUS_TIMEOUT: Duration = Duration::from_secs(30);

/// Audit action name for phone-triggered cloud inventory reads.
const KDC_CLOUD_AUDIT_ACTION: &str = "kdc_cloud";

/// The systemd cloud-arm credential is 64 hex characters plus optional
/// surrounding whitespace. Keep the loader's allocation bounded even if the
/// credential path is replaced with a hostile regular file.
const CLOUD_ARM_CREDENTIAL_MAX_BYTES: usize = 4 * 1024;

/// The placement-local cloud inventory commands the phone can trigger.
///
/// Stock KDE Connect's run-command sends only a curated key, not the bounded
/// target, generation, backend and action required by a Workload operation.
/// Consequently this surface is read-only; lifecycle belongs to Workloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CloudCommand {
    /// List every cloud provider instance (name + status).
    List,
    /// Summarize the roster (counts by status).
    Status,
}

impl CloudCommand {
    /// The curated run-command `key` for each command.
    const fn key(self) -> &'static str {
        match self {
            Self::List => "cloud-list",
            Self::Status => "cloud-status",
        }
    }

    /// The phone-visible name shown in the run-command list.
    const fn name(self) -> &'static str {
        match self {
            Self::List => "Cloud: list this node's instances",
            Self::Status => "Cloud: this node's status",
        }
    }

    /// Map a run-command key to its command, or `None` for a non-cloud key.
    pub(super) fn from_key(key: &str) -> Option<Self> {
        [Self::List, Self::Status]
            .into_iter()
            .find(|c| c.key() == key)
    }
}

/// Every cloud command as a [`RunCmd`] so it appears in the phone's run-command
/// list. The `command` field is a static label (cloud keys never shell out).
pub(super) fn cloud_command_entries() -> Vec<RunCmd> {
    [CloudCommand::List, CloudCommand::Status]
        .into_iter()
        .map(|c| RunCmd {
            key: c.key().to_string(),
            name: c.name().to_string(),
            command: "(Cloud inventory over the Bus)".to_string(),
        })
        .collect()
}

/// A phone-friendly one-line roster listing (`cloud-list`). Pure + testable.
pub(super) fn summarize_instances(instances: &[CloudInstance]) -> String {
    if instances.is_empty() {
        return "No cloud instances".to_string();
    }
    let rows: Vec<String> = instances
        .iter()
        .map(|i| format!("{} [{}]", i.name, i.status))
        .collect();
    format!("{} instance(s): {}", instances.len(), rows.join(", "))
}

/// A phone-friendly status summary — counts by state (`cloud-status`). Pure.
pub(super) fn summarize_status(instances: &[CloudInstance]) -> String {
    let active = instances
        .iter()
        .filter(|i| i.status.eq_ignore_ascii_case("ACTIVE"))
        .count();
    let shutoff = instances
        .iter()
        .filter(|i| i.status.eq_ignore_ascii_case("SHUTOFF"))
        .count();
    let other = instances.len() - active - shutoff;
    format!(
        "Cloud: {} instance(s) — {active} active, {shutoff} shutoff, {other} other",
        instances.len()
    )
}

/// One synchronous cloud Bus round-trip: publish `action/cloud/<verb>` with
/// `body` and poll `reply/<ulid>` until the provider adapter answers or
/// [`CLOUD_BUS_TIMEOUT`] elapses. Sync (the `Persist` never crosses an await —
/// it runs inside `spawn_blocking`), consuming the PUBLIC rpc + verb interface.
/// `None` is an honest gate (no responder / timeout), never a fabricated reply.
fn cloud_bus_call(persist: &Persist, verb: &str, body: &str) -> Option<CloudReply> {
    let topic = cloud_action_topic(verb);
    let ulid = publish_request(persist, &topic, Priority::Default, None, Some(body)).ok()?;
    let rtopic = reply_topic(&ulid);
    let deadline = std::time::Instant::now() + CLOUD_BUS_TIMEOUT;
    let mut cursor: Option<String> = None;
    let mut last_failure = None;
    loop {
        if let Ok(msgs) = persist.list_since(&rtopic, cursor.as_deref()) {
            for message in msgs {
                cursor = Some(message.ulid);
                let Some(reply) = message
                    .body
                    .as_deref()
                    .and_then(|raw| serde_json::from_str::<CloudReply>(raw).ok())
                    .filter(|reply| reply.verb == verb)
                else {
                    continue;
                };
                if reply.ok {
                    return Some(reply);
                }
                last_failure = Some(reply);
            }
        }
        if std::time::Instant::now() >= deadline {
            return last_failure;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Load the mint authority only from mackesd's root-only systemd credential.
/// There is no environment-secret or generated-key fallback.
pub(super) fn production_cloud_arm_signer() -> Result<CloudArmSigner, String> {
    if !rustix::process::geteuid().is_root() {
        return Err("cloud authorization requires the root mackesd service".to_string());
    }
    let directory = std::env::var_os("CREDENTIALS_DIRECTORY")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "systemd cloud arming credential is unavailable".to_string())?;
    let path = directory.join(CLOUD_ARM_CREDENTIAL);
    let raw = read_cloud_arm_credential(&path)
        .map_err(|error| format!("read systemd credential {}: {error}", path.display()))?;
    let key = decode_cloud_arm_credential(&raw).map_err(str::to_string)?;
    CloudArmSigner::new(key).map_err(str::to_string)
}

/// Read the systemd credential from a descriptor that refuses a final
/// symlink, verifies that the opened inode is a regular file, and consumes at
/// most one sentinel byte beyond the credential ceiling. The descriptor is
/// opened before metadata is checked so the inode inspected is the inode read.
fn read_cloud_arm_credential(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    #[cfg(unix)]
    let file: std::fs::File = {
        use rustix::fs::{Mode, OFlags};

        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into()
    };
    #[cfg(not(unix))]
    let file = std::fs::File::open(path)?;

    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "cloud arming credential is not a regular file",
        ));
    }
    let limit = CLOUD_ARM_CREDENTIAL_MAX_BYTES as u64;
    if metadata.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cloud arming credential exceeds {CLOUD_ARM_CREDENTIAL_MAX_BYTES}-byte limit"),
        ));
    }

    use std::io::Read as _;
    let mut raw = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(CLOUD_ARM_CREDENTIAL_MAX_BYTES)
            .saturating_add(1),
    );
    file.take(limit.saturating_add(1)).read_to_end(&mut raw)?;
    if raw.len() > CLOUD_ARM_CREDENTIAL_MAX_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cloud arming credential exceeds {CLOUD_ARM_CREDENTIAL_MAX_BYTES}-byte limit"),
        ));
    }
    Ok(raw)
}

/// Run a cloud inventory command against this placement node over the Bus and
/// return the phone-friendly result line. Sync (the `Persist` Bus round-trips
/// cannot cross an await); the async caller runs it via `spawn_blocking`.
fn run_cloud_command_blocking(cmd: CloudCommand, node: &str) -> String {
    let Some(bus) = mde_bus::default_data_dir() else {
        return "Cloud unavailable (no Bus)".to_string();
    };
    let Ok(persist) = Persist::open(bus) else {
        return "Cloud unavailable (Bus not open)".to_string();
    };
    // Read commands use a placement-scoped roster query. They never feed a
    // later privileged target decision.
    let roster_body = json!({
        "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        "node": node,
    })
    .to_string();
    let instances = match cloud_bus_call(&persist, "list-instances-local", &roster_body) {
        Some(reply) if reply.ok => reply.instances.unwrap_or_default(),
        Some(reply) => {
            return format!(
                "Cloud gated: {}",
                reply.gated.or(reply.error).unwrap_or_default()
            );
        }
        None => {
            return "Cloud unavailable (no response from this node's cloud worker)".to_string();
        }
    };
    audit_kdc_action(json!({
        "action": KDC_CLOUD_AUDIT_ACTION,
        "verb": "list-instances-local",
        "node": node,
        "count": instances.len(),
    }));
    match cmd {
        CloudCommand::Status => summarize_status(&instances),
        _ => summarize_instances(&instances),
    }
}

/// Handle a phone-triggered cloud command: run placement-local Bus round-trips off the
/// reactor (`spawn_blocking`, since `Persist` is `!Send`) + ping the result back
/// to the phone.
pub(super) async fn handle_cloud_command(
    transport: &OverlayTransport,
    peer: &PeerId,
    cmd: CloudCommand,
    node: &str,
) {
    let node = node.to_string();
    let result = tokio::task::spawn_blocking(move || run_cloud_command_blocking(cmd, &node))
        .await
        .unwrap_or_else(|_| "cloud command failed".to_string());
    info!(device = %peer.as_str(), command = cmd.key(), "kdc-host: ran phone cloud command");
    let pkt = build_packet("kdeconnect.ping", json!({ "message": result }));
    if let Err(e) = transport.send_to(peer, pkt).await {
        warn!(error = %e, "kdc-host: cloud command result ping failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_arm_credential_reader_accepts_a_regular_file() {
        let temp = tempfile::tempdir().expect("temporary credential directory");
        let path = temp.path().join(CLOUD_ARM_CREDENTIAL);
        let credential = format!(" {}\n", "ab".repeat(32));
        std::fs::write(&path, credential.as_bytes()).expect("write credential");

        assert_eq!(
            read_cloud_arm_credential(&path).expect("read credential"),
            credential.as_bytes()
        );
    }

    #[test]
    fn cloud_arm_credential_reader_rejects_oversized_regular_file() {
        let temp = tempfile::tempdir().expect("temporary credential directory");
        let path = temp.path().join(CLOUD_ARM_CREDENTIAL);
        std::fs::write(&path, vec![b'x'; CLOUD_ARM_CREDENTIAL_MAX_BYTES + 1])
            .expect("write oversized credential");

        let error = read_cloud_arm_credential(&path).expect_err("oversized credential");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeds"));
    }

    #[cfg(unix)]
    #[test]
    fn cloud_arm_credential_reader_rejects_a_final_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary credential directory");
        let target = temp.path().join("real-credential");
        let path = temp.path().join(CLOUD_ARM_CREDENTIAL);
        std::fs::write(&target, "ab".repeat(32)).expect("write credential");
        symlink(&target, &path).expect("create credential symlink");

        assert!(
            read_cloud_arm_credential(&path).is_err(),
            "credential loader must not follow a final symlink"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cloud_arm_credential_reader_rejects_non_regular_files() {
        let temp = tempfile::tempdir().expect("temporary credential directory");

        let error = read_cloud_arm_credential(temp.path()).expect_err("directory credential");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("regular file"));
    }
}
