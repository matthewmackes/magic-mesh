//! KDC-MESH-8 — fleet cloud lifecycle run-commands for the KDC host.
//!
//! Split out of the parent `kdc_host` god-file (behavior-preserving
//! relocation): the phone-triggered [`CloudCommand`] set that drives the
//! QC `action/cloud/*` typed Bus verbs by consuming the installed provider
//! adapter's PUBLIC interface, never touching the adapter. Every action audits
//! #16.

use super::*;

/// How long a phone-triggered cloud Bus round-trip waits for the provider
/// adapter's reply before honest-gating "cloud unavailable" (no fabricated
/// result).
const CLOUD_BUS_TIMEOUT: Duration = Duration::from_secs(30);

/// Audit action name for phone-triggered cloud lifecycle work.
const KDC_CLOUD_AUDIT_ACTION: &str = "kdc_cloud";

/// The fleet cloud lifecycle commands the phone can trigger (design #12).
///
/// Bulk-scoped because stock KDE Connect's run-command sends only a curated
/// `key` (no instance argument): `List`/`Status` read the roster; `StartAll`/
/// `StopAll`/`RebootAll` act on every matching instance. **Delete is deliberately
/// NOT phone-exposed** — a fleet-wide delete with no per-command confirm is past
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
            Self::List => "Cloud: list instances",
            Self::Status => "Cloud: status",
            Self::StartAll => "Cloud: start all instances",
            Self::StopAll => "Cloud: stop all instances",
            Self::RebootAll => "Cloud: reboot all instances",
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

/// The provider lifecycle Bus verb for a lifecycle action
/// (`instance-start` / `instance-stop` / `instance-reboot`).
pub(super) fn lifecycle_bus_verb(action: LifecycleAction) -> String {
    format!("instance-{}", action.cli_verb())
}

/// Pick the instances a bulk lifecycle command acts on, filtered by provider
/// status:
/// `Start` targets `SHUTOFF` instances, `Stop`/`Reboot` target `ACTIVE` ones,
/// `Delete` (never phone-exposed) targets none. Pure + testable — the decision
/// that keeps a start-all from redundantly starting already-running instances.
pub(super) fn plan_cloud_lifecycle(
    action: LifecycleAction,
    instances: &[CloudInstance],
) -> Vec<String> {
    instances
        .iter()
        .filter(|i| match action {
            LifecycleAction::Start => i.status.eq_ignore_ascii_case("SHUTOFF"),
            LifecycleAction::Stop | LifecycleAction::Reboot => {
                i.status.eq_ignore_ascii_case("ACTIVE")
            }
            LifecycleAction::Delete => false,
        })
        .map(|i| i.name.clone())
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
    loop {
        if let Ok(msgs) = persist.list_since(&rtopic, None) {
            if let Some(m) = msgs.first() {
                return m
                    .body
                    .as_deref()
                    .and_then(|b| serde_json::from_str::<CloudReply>(b).ok());
            }
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Run a cloud command against the fleet over the Bus (design #12) and return the
/// phone-friendly result line. Sync (the `Persist` Bus round-trips can't cross an
/// await); the async caller runs it via `spawn_blocking`. Every performed op
/// audits (#16); an unavailable cloud / no-responder is an honest gate.
fn run_cloud_command_blocking(cmd: CloudCommand) -> String {
    let Some(bus) = mde_bus::default_data_dir() else {
        return "Cloud unavailable (no Bus)".to_string();
    };
    let Ok(persist) = Persist::open(bus) else {
        return "Cloud unavailable (Bus not open)".to_string();
    };
    // Every cloud command starts from the live instance roster.
    let instances = match cloud_bus_call(&persist, "list-instances", "{}") {
        Some(reply) if reply.ok => reply.instances.unwrap_or_default(),
        Some(reply) => {
            return format!(
                "Cloud gated: {}",
                reply.gated.or(reply.error).unwrap_or_default()
            );
        }
        None => {
            return "Cloud unavailable (no response from a cloud provider adapter)".to_string();
        }
    };
    audit_kdc_action(json!({
        "action": KDC_CLOUD_AUDIT_ACTION,
        "verb": "list-instances",
        "count": instances.len(),
    }));
    let Some(action) = cmd.lifecycle() else {
        // A read command — summarize the roster.
        return match cmd {
            CloudCommand::Status => summarize_status(&instances),
            _ => summarize_instances(&instances),
        };
    };
    let targets = plan_cloud_lifecycle(action, &instances);
    if targets.is_empty() {
        return format!("{}: no matching instances", cmd.name());
    }
    let verb = lifecycle_bus_verb(action);
    let (mut done, mut failed) = (0_usize, 0_usize);
    for name in &targets {
        let body = json!({ "instance": name }).to_string();
        match cloud_bus_call(&persist, &verb, &body) {
            Some(r) if r.ok => {
                done += 1;
                audit_kdc_action(json!({
                    "action": KDC_CLOUD_AUDIT_ACTION,
                    "verb": verb,
                    "instance": name,
                    "audited": r.audited,
                }));
            }
            Some(r) => {
                failed += 1;
                audit_kdc_action(json!({
                    "action": KDC_CLOUD_AUDIT_ACTION,
                    "verb": verb,
                    "instance": name,
                    "result": "failed",
                    "reason": r.error.or(r.gated).unwrap_or_default(),
                }));
            }
            None => {
                failed += 1;
                audit_kdc_action(json!({
                    "action": KDC_CLOUD_AUDIT_ACTION,
                    "verb": verb,
                    "instance": name,
                    "result": "timeout",
                }));
            }
        }
    }
    format!(
        "{}: {done} ok, {failed} failed (of {})",
        cmd.name(),
        targets.len()
    )
}

/// Handle a phone-triggered cloud command: run the fleet Bus round-trips off the
/// reactor (`spawn_blocking`, since `Persist` is `!Send`) + ping the result back
/// to the phone.
pub(super) async fn handle_cloud_command(
    transport: &OverlayTransport,
    peer: &PeerId,
    cmd: CloudCommand,
) {
    let result = tokio::task::spawn_blocking(move || run_cloud_command_blocking(cmd))
        .await
        .unwrap_or_else(|_| "cloud command failed".to_string());
    info!(device = %peer.as_str(), command = cmd.key(), "kdc-host: ran phone cloud command");
    let pkt = build_packet("kdeconnect.ping", json!({ "message": result }));
    if let Err(e) = transport.send_to(peer, pkt).await {
        warn!(error = %e, "kdc-host: cloud command result ping failed");
    }
}
