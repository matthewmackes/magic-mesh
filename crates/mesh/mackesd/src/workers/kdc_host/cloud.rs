//! KDC-MESH-8 — placement-local cloud lifecycle run-commands for the KDC host.
//!
//! Split out of the parent `kdc_host` god-file (behavior-preserving
//! relocation): the phone-triggered [`CloudCommand`] set that drives the
//! QC `action/cloud/*` typed Bus verbs by consuming the installed provider
//! adapter's PUBLIC interface, never touching the adapter. Every action audits
//! #16.

use super::*;
use mackes_mesh_types::cloud::{
    cloud_request_digest, decode_cloud_arm_credential, CloudArmSigner, CloudArmedToken,
    CLOUD_ACTION_SCHEMA_VERSION, CLOUD_ARM_CREDENTIAL, CLOUD_ARM_NODE_SCOPE,
};

/// A phone capability remains useful long enough for one local Bus drain, but
/// not as a reusable ambient credential.
const CLOUD_ARM_TTL_MS: i64 = 30_000;

/// How long a phone-triggered cloud Bus round-trip waits for the provider
/// adapter's reply before honest-gating "cloud unavailable" (no fabricated
/// result).
const CLOUD_BUS_TIMEOUT: Duration = Duration::from_secs(30);

/// Audit action name for phone-triggered cloud lifecycle work.
const KDC_CLOUD_AUDIT_ACTION: &str = "kdc_cloud";

/// The placement-local cloud lifecycle commands the phone can trigger (design #12).
///
/// Bulk-scoped because stock KDE Connect's run-command sends only a curated
/// `key` (no instance argument): `List`/`Status` read the roster; `StartAll`/
/// `StopAll`/`RebootAll` act on every matching instance on this KDC node. **Delete
/// is deliberately NOT phone-exposed** — a bulk delete with no typed target is past
/// the safety line the audit log alone shouldn't backstop (a targeted delete
/// needs an instance the stock run-command can't carry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CloudCommand {
    /// List every cloud provider instance (name + status).
    List,
    /// Summarize the roster (counts by status).
    Status,
    /// Start every `SHUTOFF` instance.
    StartAll,
    /// Stop every `ACTIVE` instance.
    StopAll,
    /// Reboot every `ACTIVE` instance.
    RebootAll,
}

impl CloudCommand {
    /// The curated run-command `key` for each command.
    const fn key(self) -> &'static str {
        match self {
            Self::List => "cloud-list",
            Self::Status => "cloud-status",
            Self::StartAll => "cloud-start-all",
            Self::StopAll => "cloud-stop-all",
            Self::RebootAll => "cloud-reboot-all",
        }
    }

    /// The phone-visible name shown in the run-command list.
    const fn name(self) -> &'static str {
        match self {
            Self::List => "Cloud: list this node's instances",
            Self::Status => "Cloud: this node's status",
            Self::StartAll => "Cloud: start stopped instances on this node",
            Self::StopAll => "Cloud: stop active instances on this node",
            Self::RebootAll => "Cloud: reboot active instances on this node",
        }
    }

    /// Map a run-command key to its command, or `None` for a non-cloud key.
    pub(super) fn from_key(key: &str) -> Option<Self> {
        [
            Self::List,
            Self::Status,
            Self::StartAll,
            Self::StopAll,
            Self::RebootAll,
        ]
        .into_iter()
        .find(|c| c.key() == key)
    }

    /// The lifecycle action a bulk command drives (`None` for the reads).
    const fn lifecycle(self) -> Option<LifecycleAction> {
        match self {
            Self::StartAll => Some(LifecycleAction::Start),
            Self::StopAll => Some(LifecycleAction::Stop),
            Self::RebootAll => Some(LifecycleAction::Reboot),
            Self::List | Self::Status => None,
        }
    }
}

/// Every cloud command as a [`RunCmd`] so it appears in the phone's run-command
/// list. The `command` field is a static label (cloud keys never shell out — they
/// route through the Bus in [`handle_cloud_command`]).
pub(super) fn cloud_command_entries() -> Vec<RunCmd> {
    [
        CloudCommand::List,
        CloudCommand::Status,
        CloudCommand::StartAll,
        CloudCommand::StopAll,
        CloudCommand::RebootAll,
    ]
    .into_iter()
    .map(|c| RunCmd {
        key: c.key().to_string(),
        name: c.name().to_string(),
        command: "(Cloud provider lifecycle over the Bus)".to_string(),
    })
    .collect()
}

/// The placement-local bulk lifecycle Bus verb. Target selection happens in
/// the privileged cloud worker from its live runner roster, never in this
/// public-Bus client.
pub(super) fn lifecycle_bulk_bus_verb(action: LifecycleAction) -> String {
    format!("instance-{}-all", action.cli_verb())
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

/// Insert a short-lived, body-bound capability into one frozen placement-local
/// bulk request. Pure apart from its supplied clock/nonce so hostile-body tests
/// do not need a production credential.
pub(super) fn authorize_bulk_body_with_signer(
    signer: &CloudArmSigner,
    node: &str,
    verb: &str,
    now_ms: i64,
    nonce: &str,
) -> Result<String, String> {
    for (label, value) in [("node", node), ("verb", verb), ("nonce", nonce)] {
        if value.is_empty() || value.len() > 255 || value.contains('|') {
            return Err(format!(
                "cloud authorization {label} is not capability-safe"
            ));
        }
    }
    let body = json!({
        "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        "node": node,
    })
    .to_string();
    let token = CloudArmedToken::mint(
        signer,
        nonce,
        now_ms.saturating_add(CLOUD_ARM_TTL_MS),
        verb,
        node,
        CLOUD_ARM_NODE_SCOPE,
        &cloud_request_digest(&body).map_err(str::to_string)?,
    )
    .encode();
    let mut document: Value =
        serde_json::from_str(&body).map_err(|error| format!("build cloud request: {error}"))?;
    document["armed_token"] = Value::String(token);
    Ok(document.to_string())
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
    let raw = std::fs::read(&path)
        .map_err(|error| format!("read systemd credential {}: {error}", path.display()))?;
    let key = decode_cloud_arm_credential(&raw).map_err(str::to_string)?;
    CloudArmSigner::new(key).map_err(str::to_string)
}

fn authorize_bulk_body(node: &str, verb: &str) -> Result<String, String> {
    use rand::RngCore as _;
    let signer = production_cloud_arm_signer()?;
    let mut nonce = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let nonce = nonce
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .map_err(|_| "system clock is before the Unix epoch".to_string())?;
    authorize_bulk_body_with_signer(&signer, node, verb, now_ms, &nonce)
}

/// Run a cloud command against this placement node over the Bus (design #12) and return the
/// phone-friendly result line. Sync (the `Persist` Bus round-trips can't cross an
/// await); the async caller runs it via `spawn_blocking`. Every performed op
/// audits (#16); an unavailable cloud / no-responder is an honest gate.
fn run_cloud_command_blocking(cmd: CloudCommand, node: &str) -> String {
    let Some(bus) = mde_bus::default_data_dir() else {
        return "Cloud unavailable (no Bus)".to_string();
    };
    let Ok(persist) = Persist::open(bus) else {
        return "Cloud unavailable (Bus not open)".to_string();
    };
    if let Some(action) = cmd.lifecycle() {
        let verb = lifecycle_bulk_bus_verb(action);
        let body = match authorize_bulk_body(node, &verb) {
            Ok(body) => body,
            Err(error) => return format!("Cloud authorization unavailable: {error}"),
        };
        let result = match cloud_bus_call(&persist, &verb, &body) {
            Some(reply) if reply.ok => reply
                .raw_log
                .unwrap_or_else(|| format!("{} completed", cmd.name())),
            Some(reply) => format!(
                "Cloud gated: {}",
                reply.gated.or(reply.error).unwrap_or_default()
            ),
            None => "Cloud unavailable (no response from this node's cloud worker)".to_string(),
        };
        audit_kdc_action(json!({
            "action": KDC_CLOUD_AUDIT_ACTION,
            "verb": verb,
            "node": node,
            "result": &result,
        }));
        return result;
    }

    // Read commands use a placement-scoped roster query. They never feed a
    // later privileged target decision; mutations are worker-selected above.
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
