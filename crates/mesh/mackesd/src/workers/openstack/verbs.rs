//! QC-11 (QUASAR-CLOUD) — the typed Bus verb surface that wraps the cloud.
//!
//! Design Q40/Q70: the mackesd Bus verbs stay §9's contract; `OpenStack` is the
//! backend, and no mesh client (the shell Cloud plane, the phone via
//! KDC-MESH-8, `meshctl`) ever speaks raw `openstack`. This module is that
//! contract for the `openstack` worker: typed `action/cloud/*` requests →
//! typed [`CloudReply`], served on `reply/<ulid>` with the same request/reply
//! idiom the `unit_aggregator`'s `action/units/get-stream` verb uses
//! (`crates/platform/mde-bus/src/rpc.rs`).
//!
//! ## The verbs (`action/cloud/<verb>`)
//!
//! - **Reads (audit-exempt — the `get-`/`list-` prefix marks them
//!   observational, [`mde_bus::persist::is_auditable`]):**
//!   - `get-status` — the whole [`OpenStackState`] mirror (doctrine + runtime +
//!     per-service rows + extras) this node last converged.
//!   - `list-services` — just the per-service rows (the focused service view).
//!   - `list-instances` — the Nova instance roster (drives the real seam).
//! - **Lifecycle (audited mutations):** `instance-start` / `instance-stop` /
//!   `instance-reboot` / `instance-delete` — the ops KDC-MESH-8's phone
//!   run-commands + the Cloud plane call.
//!
//! ## Honest, never fake (§7)
//!
//! The reads fold the **real** [`OpenStackState`] the reconcile loop maintains
//! (QC-2..10). Every seam-touching verb ([`list-instances`] + the four
//! lifecycle verbs) first passes [`cloud_gate`]: the doctrine must be
//! `Enabled`, the container runtime `Available`, and `nova_api` `Running` —
//! else a typed **gated** reply (retry later), never a fabricated success.
//! Only past the gate is the real [`InstanceOps`] seam driven; a seam failure
//! is a typed **failed** reply carrying the CLI's error. The destructive verbs
//! (`instance-delete` / `instance-reboot`) are therefore performed **only when
//! the doctrine is enabled and the service is running**, and each performed
//! destructive op is audited (the request topic hash-chains via
//! `is_auditable`, and the responder logs it on `mackesd::openstack`).
//!
//! ## The seam (§6 — reuse the one process vocabulary)
//!
//! [`OpenstackCli`] shells `openstack server …` through the same bounded
//! [`crate::workers::proc`] path the QC-2 [`super::podman::PodmanCli`] uses
//! (a wedged CLI can't pin a runtime thread); a host without
//! `python-openstackclient` (Q27) answers a typed
//! [`InstanceOpError::CliAbsent`]. Tests drive the in-memory
//! [`super::testkit::FakeInstanceOps`].

use std::collections::BTreeMap;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

use crate::workers::proc::output_with_timeout;

use super::catalog::ServiceKind;
use super::reconcile::{DoctrineStatus, OpenStackState, RuntimeStatus, ServiceRow, ServiceStatus};

// ─────────────────────────── the verb vocabulary ───────────────────────────

/// The `action/cloud/` namespace prefix every cloud verb request rides.
pub const CLOUD_ACTION_PREFIX: &str = "action/cloud/";

/// Every typed cloud verb, in the order the responder drains them.
///
/// The read verbs carry a `get-`/`list-` stem so they are audit-exempt
/// ([`mde_bus::persist::is_auditable`], the BUS-AUDIT-FLOOD guard); the four
/// `instance-*` lifecycle verbs are control-plane mutations and audit.
pub const CLOUD_VERBS: [&str; 7] = [
    "get-status",
    "list-services",
    "list-instances",
    "instance-start",
    "instance-stop",
    "instance-reboot",
    "instance-delete",
];

/// The Bus topic for cloud verb `verb`: `action/cloud/<verb>`.
#[must_use]
pub fn cloud_action_topic(verb: &str) -> String {
    format!("{CLOUD_ACTION_PREFIX}{verb}")
}

/// A Nova instance lifecycle action a typed verb drives through the seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    /// `openstack server start`.
    Start,
    /// `openstack server stop`.
    Stop,
    /// `openstack server reboot` (soft reboot) — destructive.
    Reboot,
    /// `openstack server delete` — destructive.
    Delete,
}

impl LifecycleAction {
    /// Map a lifecycle verb name to its action, or `None` for a non-lifecycle
    /// verb.
    #[must_use]
    pub fn from_verb(verb: &str) -> Option<Self> {
        match verb {
            "instance-start" => Some(Self::Start),
            "instance-stop" => Some(Self::Stop),
            "instance-reboot" => Some(Self::Reboot),
            "instance-delete" => Some(Self::Delete),
            _ => None,
        }
    }

    /// The `openstack server <verb>` sub-verb.
    #[must_use]
    pub const fn cli_verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Reboot => "reboot",
            Self::Delete => "delete",
        }
    }

    /// Whether performing this op is destructive (delete/reboot) — the ops that
    /// are only ever run past the gate and are audited when performed (§7).
    #[must_use]
    pub const fn is_destructive(self) -> bool {
        matches!(self, Self::Reboot | Self::Delete)
    }
}

// ─────────────────────────── the instance seam ───────────────────────────

/// A cloud instance as the Nova seam reports it — the typed row the
/// `list-instances` verb returns (the Cloud plane's instance table, QC-12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudInstance {
    /// The Nova server id (UUID).
    pub id: String,
    /// The server name.
    pub name: String,
    /// The Nova status (`ACTIVE` / `SHUTOFF` / `ERROR` / …).
    pub status: String,
    /// The flavor name/id, when the listing carried it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flavor: Option<String>,
    /// The image name/id, when the listing carried it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// The networks column, rendered to a string, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub networks: Option<String>,
}

/// An `openstack` CLI failure — a typed, honest degrade (never a fake success).
#[derive(Debug, Error)]
pub enum InstanceOpError {
    /// `python-openstackclient` is not on this host at all (Q27 ships it in the
    /// host image; a dev/CI box without it degrades honestly).
    #[error("openstack CLI absent: {reason}")]
    CliAbsent {
        /// Why/where it was expected.
        reason: String,
    },
    /// The `openstack` process couldn't be spawned (other than absence) or
    /// timed out.
    #[error("spawn openstack: {0}")]
    Spawn(String),
    /// A command exited non-zero — the sub-command, exit code, and stderr.
    #[error("openstack {cmd} failed (exit {code}): {stderr}")]
    Command {
        /// The failing sub-command (e.g. `server start`).
        cmd: String,
        /// Process exit code (or -1 if killed by signal).
        code: i32,
        /// Captured stderr.
        stderr: String,
    },
    /// `openstack server list -f json` output didn't parse.
    #[error("parsing `openstack server list` output failed: {0}")]
    Parse(String),
}

/// The injectable Nova instance seam (QC-11). [`OpenstackCli`] is the
/// production impl; tests wire [`super::testkit::FakeInstanceOps`].
///
/// Deliberately the *lifecycle + list* slice the MVP verbs need (Q79
/// boot-attach-reach's lifecycle half). The launch/attach picker (QC-12/Q83)
/// grows the seam in place, exactly as the QC catalog grows.
pub trait InstanceOps {
    /// List the cloud's Nova instances (`openstack server list -f json`).
    ///
    /// # Errors
    /// [`InstanceOpError::CliAbsent`] on a CLI-less host; spawn / non-zero /
    /// parse failures otherwise.
    fn list(&self) -> Result<Vec<CloudInstance>, InstanceOpError>;

    /// Perform `action` on the instance named `instance`
    /// (`openstack server <verb> <instance>`).
    ///
    /// # Errors
    /// [`InstanceOpError::CliAbsent`] / spawn / non-zero failures.
    fn perform(&self, action: LifecycleAction, instance: &str) -> Result<(), InstanceOpError>;
}

// ─────────────────── pure: openstack argv builders + parse ───────────────────
// Each returns the argv WITHOUT the leading `openstack`, pure + pinned by tests
// so the command surface can't silently drift (mirrors `podman`'s builders).

/// The `openstack server list -f json` argv.
#[must_use]
pub fn build_server_list_argv() -> Vec<String> {
    vec!["server".into(), "list".into(), "-f".into(), "json".into()]
}

/// The `openstack server <verb> <instance>` lifecycle argv.
///
/// `instance` rides as its own argv element (never shell-interpolated), so a
/// hostile id can't inject a command; an empty id is caller-rejected upstream.
#[must_use]
pub fn build_lifecycle_argv(action: LifecycleAction, instance: &str) -> Vec<String> {
    vec![
        "server".into(),
        action.cli_verb().into(),
        instance.into(),
    ]
}

/// Parse `openstack server list -f json` output into the typed roster.
///
/// The CLI emits a JSON array of objects with capitalized columns
/// (`ID`/`Name`/`Status`/`Flavor`/`Image`/`Networks`); `Networks` may be a
/// string or a nested object, so it's rendered to a compact string. Missing
/// optional columns stay `None` (§7 — never guessed).
///
/// # Errors
/// [`InstanceOpError::Parse`] when the body isn't the expected JSON array.
pub fn parse_server_list_json(body: &str) -> Result<Vec<CloudInstance>, InstanceOpError> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(rename = "ID")]
        id: String,
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "Status")]
        status: String,
        #[serde(rename = "Flavor", default)]
        flavor: Option<String>,
        #[serde(rename = "Image", default)]
        image: Option<String>,
        #[serde(rename = "Networks", default)]
        networks: Option<serde_json::Value>,
    }
    let raws: Vec<Raw> =
        serde_json::from_str(body.trim()).map_err(|e| InstanceOpError::Parse(e.to_string()))?;
    Ok(raws
        .into_iter()
        .map(|r| CloudInstance {
            id: r.id,
            name: r.name,
            status: r.status,
            flavor: r.flavor.filter(|s| !s.is_empty()),
            image: r.image.filter(|s| !s.is_empty()),
            networks: r.networks.and_then(|v| match v {
                serde_json::Value::Null => None,
                serde_json::Value::String(s) if s.is_empty() => None,
                serde_json::Value::String(s) => Some(s),
                other => Some(other.to_string()),
            }),
        })
        .collect())
}

/// The bound on one `openstack` CLI call.
///
/// Generous — the CLI mints a Keystone token + hits Nova over the overlay,
/// slower than a local `podman` call — but still frees the worker's blocking
/// thread if the API truly wedges.
pub const OPENSTACK_CMD_TIMEOUT: Duration = Duration::from_secs(60);

/// Production [`InstanceOps`]: shells `openstack` through the bounded
/// [`crate::workers::proc`] path.
///
/// Stateless — every call a fresh bounded process, authed off the host's
/// rendered `clouds.yaml` (invisible SSO, Q87).
#[derive(Debug, Clone, Default)]
pub struct OpenstackCli;

impl OpenstackCli {
    /// Construct the production `openstack` CLI seam.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Run `openstack <args>` capturing stdout/stderr + exit code, bounded.
    fn run(args: &[String]) -> Result<(i32, String, String), InstanceOpError> {
        let mut cmd = Command::new("openstack");
        cmd.args(args);
        let out = output_with_timeout(cmd, OPENSTACK_CMD_TIMEOUT).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                InstanceOpError::CliAbsent {
                    reason: "the `openstack` binary is not on PATH — the MCNF host image \
                             ships python-openstackclient (design Q27); a dev/CI box \
                             without it degrades honestly"
                        .to_string(),
                }
            } else {
                InstanceOpError::Spawn(e.to_string())
            }
        })?;
        Ok((
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }
}

impl InstanceOps for OpenstackCli {
    fn list(&self) -> Result<Vec<CloudInstance>, InstanceOpError> {
        let (code, stdout, stderr) = Self::run(&build_server_list_argv())?;
        if code != 0 {
            return Err(InstanceOpError::Command {
                cmd: "server list".into(),
                code,
                stderr: stderr.trim().to_string(),
            });
        }
        parse_server_list_json(&stdout)
    }

    fn perform(&self, action: LifecycleAction, instance: &str) -> Result<(), InstanceOpError> {
        let (code, _stdout, stderr) = Self::run(&build_lifecycle_argv(action, instance))?;
        if code == 0 {
            Ok(())
        } else {
            Err(InstanceOpError::Command {
                cmd: format!("server {}", action.cli_verb()),
                code,
                stderr: stderr.trim().to_string(),
            })
        }
    }
}

// ─────────────────────────── typed request/reply ───────────────────────────

/// The typed request body for a lifecycle verb — which instance to act on.
///
/// The read verbs take an empty body; the lifecycle verbs require a non-empty
/// `instance` (the Nova server id/name).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct InstanceRequest {
    /// The Nova server id (UUID) or name to act on.
    pub instance: String,
}

/// Parse a lifecycle request body into a typed [`InstanceRequest`].
///
/// # Errors
/// A human-readable message when the body isn't valid request JSON.
pub fn parse_instance_request(body: &str) -> Result<InstanceRequest, String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok(InstanceRequest::default());
    }
    serde_json::from_str(trimmed).map_err(|e| format!("bad cloud request body: {e}"))
}

/// The unified typed reply published to `reply/<request-ulid>` for every
/// `action/cloud/*` verb.
///
/// `ok` mirrors the shared `{"ok":true}` reply convention (the `units`/`dc/*`
/// lanes) so a generic client classifies success without knowing the schema.
/// Exactly one payload field is set on success; a rejected/gated/failed request
/// carries `error`/`gated` and no payload (§7 — no fabricated answer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudReply {
    /// `true` when a payload answers the request; `false` on gate/failure/
    /// rejection.
    pub ok: bool,
    /// The verb this reply answers (echoed for the client's dispatch).
    pub verb: String,
    /// `get-status` — the whole node mirror.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<OpenStackState>,
    /// `list-services` — the per-service rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub services: Option<Vec<ServiceRow>>,
    /// `list-instances` — the Nova instance roster.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instances: Option<Vec<CloudInstance>>,
    /// The instance a lifecycle verb acted on, on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// An honest gate reason (the cloud isn't in a state to serve this verb —
    /// doctrine disabled/gated, runtime down, nova not running). Retry later;
    /// nothing was performed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gated: Option<String>,
    /// A rejection (malformed request) or a seam failure (the CLI errored).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether a destructive op (delete/reboot) was performed + audited.
    pub audited: bool,
}

impl CloudReply {
    fn base(verb: &str, ok: bool) -> Self {
        Self {
            ok,
            verb: verb.to_string(),
            status: None,
            services: None,
            instances: None,
            instance: None,
            gated: None,
            error: None,
            audited: false,
        }
    }

    /// `get-status` — the whole mirror.
    #[must_use]
    pub fn status(verb: &str, state: OpenStackState) -> Self {
        Self {
            status: Some(state),
            ..Self::base(verb, true)
        }
    }

    /// `list-services` — the per-service rows.
    #[must_use]
    pub fn services(verb: &str, services: Vec<ServiceRow>) -> Self {
        Self {
            services: Some(services),
            ..Self::base(verb, true)
        }
    }

    /// `list-instances` — the Nova roster.
    #[must_use]
    pub fn instances(verb: &str, instances: Vec<CloudInstance>) -> Self {
        Self {
            instances: Some(instances),
            ..Self::base(verb, true)
        }
    }

    /// A lifecycle op succeeded on `instance` (`audited` when it was
    /// destructive).
    #[must_use]
    pub fn performed(verb: &str, instance: impl Into<String>, audited: bool) -> Self {
        Self {
            instance: Some(instance.into()),
            audited,
            ..Self::base(verb, true)
        }
    }

    /// An honest gate — the cloud can't serve this verb yet; nothing performed.
    #[must_use]
    pub fn gated(verb: &str, reason: impl Into<String>) -> Self {
        Self {
            gated: Some(reason.into()),
            ..Self::base(verb, false)
        }
    }

    /// A seam failure — the op reached the CLI and it errored.
    #[must_use]
    pub fn failed(verb: &str, reason: impl Into<String>) -> Self {
        Self {
            error: Some(reason.into()),
            ..Self::base(verb, false)
        }
    }

    /// A typed rejection — a malformed/unknown request.
    #[must_use]
    pub fn rejected(verb: &str, reason: impl Into<String>) -> Self {
        Self {
            error: Some(reason.into()),
            ..Self::base(verb, false)
        }
    }

    /// JSON body for the `reply/<ulid>` lane. Infallible — a serialize failure
    /// degrades to a typed error body.
    #[must_use]
    pub fn to_body(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| r#"{"ok":false,"error":"cloud reply encode failed"}"#.to_string())
    }
}

// ─────────────────────────── the gate + dispatch ───────────────────────────

/// Render a service's honest status to a short reason fragment.
fn describe_status(status: &ServiceStatus) -> String {
    match status {
        ServiceStatus::Running => "running".to_string(),
        ServiceStatus::NotRunning { podman_state } => format!("not running ({podman_state})"),
        ServiceStatus::Gated { reason } => format!("gated ({reason})"),
        ServiceStatus::Failed { reason } => format!("failed ({reason})"),
        ServiceStatus::Unknown { reason } => format!("unknown ({reason})"),
    }
}

/// The honesty gate every seam-touching verb passes before the real
/// [`InstanceOps`] seam is driven (§7).
///
/// The doctrine must be `Enabled`, the container runtime `Available`, and
/// `nova_api` `Running` — the Nova compute API the `openstack` CLI talks to.
/// Any failing precondition is a typed, named gate reason (retry later), never
/// a fabricated success.
///
/// # Errors
/// The gate reason when the cloud isn't in a state to serve a cloud op here.
pub fn cloud_gate(state: &OpenStackState) -> Result<(), String> {
    match &state.doctrine {
        DoctrineStatus::Enabled { .. } => {}
        DoctrineStatus::Disabled => {
            return Err(format!(
                "the cloud doctrine is disabled on {} — no OpenStack services run here",
                state.host
            ))
        }
        DoctrineStatus::Gated { reason } => {
            return Err(format!("the cloud doctrine is unread on {} — {reason}", state.host))
        }
    }
    if let RuntimeStatus::Unavailable { reason } = &state.runtime {
        return Err(format!("the container runtime is unavailable on {} — {reason}", state.host));
    }
    let nova = ServiceKind::NovaApi.container_name();
    match state.services.iter().find(|r| r.service == nova) {
        Some(row) if matches!(row.status, ServiceStatus::Running) => Ok(()),
        Some(row) => Err(format!(
            "the Nova compute API ({nova}) is {} on {} — instance ops are unavailable until it converges",
            describe_status(&row.status),
            state.host
        )),
        None => Err(format!(
            "the Nova compute API ({nova}) is not a desired service on {} — this node doesn't carry the cloud control plane",
            state.host
        )),
    }
}

/// The pure verb handler.
///
/// Dispatch one `action/cloud/<verb>` request against the current mirror
/// `state` + the injected `ops` seam, answering a typed [`CloudReply`]. Reads
/// fold the real state model; seam-touching verbs pass [`cloud_gate`] first; a
/// malformed body / unknown verb is a typed rejection — never a panic, never a
/// fabricated answer (§7). Destructive ops (delete/reboot) that are actually
/// performed are logged on `mackesd::openstack` (the request topic already
/// hash-chains via [`mde_bus::persist::is_auditable`]).
#[must_use]
pub fn handle_cloud_request(
    verb: &str,
    body: &str,
    state: &OpenStackState,
    ops: &dyn InstanceOps,
) -> CloudReply {
    match verb {
        "get-status" => CloudReply::status(verb, state.clone()),
        "list-services" => CloudReply::services(verb, state.services.clone()),
        "list-instances" => match cloud_gate(state) {
            Err(reason) => CloudReply::gated(verb, reason),
            Ok(()) => match ops.list() {
                Ok(list) => CloudReply::instances(verb, list),
                Err(e) => CloudReply::failed(verb, e.to_string()),
            },
        },
        other => LifecycleAction::from_verb(other).map_or_else(
            || CloudReply::rejected(verb, format!("unknown cloud verb: {other}")),
            |action| handle_lifecycle(verb, action, body, state, ops),
        ),
    }
}

/// The lifecycle leg of [`handle_cloud_request`]: parse the target instance,
/// pass [`cloud_gate`], drive the seam, and audit a performed destructive op.
fn handle_lifecycle(
    verb: &str,
    action: LifecycleAction,
    body: &str,
    state: &OpenStackState,
    ops: &dyn InstanceOps,
) -> CloudReply {
    let req = match parse_instance_request(body) {
        Ok(r) => r,
        Err(e) => return CloudReply::rejected(verb, e),
    };
    let instance = req.instance.trim();
    if instance.is_empty() {
        return CloudReply::rejected(
            verb,
            "a cloud lifecycle verb requires a non-empty `instance` (the Nova server id)",
        );
    }
    // §7 — the gate enforces "enabled + running", so a destructive op is
    // performed ONLY when the doctrine is enabled and nova is running.
    if let Err(reason) = cloud_gate(state) {
        return CloudReply::gated(verb, reason);
    }
    match ops.perform(action, instance) {
        Ok(()) => {
            let audited = action.is_destructive();
            if audited {
                tracing::info!(
                    target: "mackesd::openstack",
                    verb,
                    instance,
                    "performed destructive cloud lifecycle op (audited)"
                );
            }
            CloudReply::performed(verb, instance, audited)
        }
        Err(e) => CloudReply::failed(verb, e.to_string()),
    }
}

/// Drain net-new `action/cloud/*` requests and answer each on `reply/<ulid>`.
///
/// Answers each net-new request since the per-verb cursors with a typed
/// [`CloudReply`] over `state` + the `ops` seam. Best-effort — a read/write
/// failure is logged and the cursor still advances (a stale request never
/// re-answers, and a lifecycle op never re-performs).
///
/// Synchronous + seam-pure: the worker drives it on a blocking task (an
/// `openstack` shell-out never pins the async runtime); tests drive it directly
/// with a real [`Persist`] tempdir + [`super::testkit::FakeInstanceOps`].
pub fn drain_cloud_verbs(
    persist: &Persist,
    cursors: &mut BTreeMap<String, Option<String>>,
    state: &OpenStackState,
    ops: &dyn InstanceOps,
) {
    for verb in CLOUD_VERBS {
        let topic = cloud_action_topic(verb);
        let cursor = cursors.entry(topic.clone()).or_default();
        let msgs = match persist.list_since(&topic, cursor.as_deref()) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(target: "mackesd::openstack", %topic, error = %e, "cloud verb: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            *cursor = Some(msg.ulid.clone());
            let body = msg.body.unwrap_or_default();
            let reply = handle_cloud_request(verb, &body, state, ops);
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply.to_body()),
            ) {
                tracing::warn!(target: "mackesd::openstack", ulid = %msg.ulid, error = %e, "cloud verb: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::FakeInstanceOps;
    use super::*;

    /// A mirror state with the doctrine `Enabled` and `nova_api` `Running` — the
    /// happy path the seam-touching verbs need.
    fn enabled_state() -> OpenStackState {
        OpenStackState {
            host: "node-a".into(),
            doctrine: DoctrineStatus::Enabled {
                leader: true,
                kolla_release: "2024.1".into(),
            },
            runtime: RuntimeStatus::Available,
            services: vec![
                ServiceRow {
                    service: "keystone".into(),
                    status: ServiceStatus::Running,
                },
                ServiceRow {
                    service: "nova_api".into(),
                    status: ServiceStatus::Running,
                },
            ],
            extras: vec![],
            published_at_ms: 7,
        }
    }

    fn sample_instances() -> Vec<CloudInstance> {
        vec![CloudInstance {
            id: "i-1".into(),
            name: "web".into(),
            status: "ACTIVE".into(),
            flavor: Some("m1.small".into()),
            image: None,
            networks: Some("flat=10.0.0.5".into()),
        }]
    }

    // ── the verb vocabulary ──

    #[test]
    fn topics_are_namespaced_and_reads_are_audit_exempt() {
        assert_eq!(cloud_action_topic("get-status"), "action/cloud/get-status");
        // The read verbs carry the audit-exempt stem; the mutations audit.
        for read in ["get-status", "list-services", "list-instances"] {
            let t = cloud_action_topic(read);
            assert!(
                !mde_bus::persist::is_auditable(&t),
                "read verb {read} must be audit-exempt (BUS-AUDIT-FLOOD)"
            );
        }
        for mutation in [
            "instance-start",
            "instance-stop",
            "instance-reboot",
            "instance-delete",
        ] {
            let t = cloud_action_topic(mutation);
            assert!(
                mde_bus::persist::is_auditable(&t),
                "lifecycle verb {mutation} must audit"
            );
        }
    }

    // ── reads fold the real state model ──

    #[test]
    fn get_status_returns_the_whole_mirror() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let reply = handle_cloud_request("get-status", "", &state, &ops);
        assert!(reply.ok);
        assert_eq!(reply.status.expect("status").host, "node-a");
        assert!(ops.calls().is_empty(), "a read never touches the seam");
    }

    #[test]
    fn list_services_returns_the_service_rows() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let reply = handle_cloud_request("list-services", "{}", &state, &ops);
        assert!(reply.ok);
        let rows = reply.services.expect("services");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.service == "nova_api"));
    }

    // ── list-instances drives the real seam / honest-gates ──

    #[test]
    fn list_instances_drives_the_seam_when_the_cloud_is_up() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new().with_instances(sample_instances());
        let reply = handle_cloud_request("list-instances", "", &state, &ops);
        assert!(reply.ok);
        assert_eq!(reply.instances.expect("instances")[0].id, "i-1");
        assert_eq!(ops.calls(), vec!["list".to_string()]);
    }

    #[test]
    fn list_instances_gates_when_the_doctrine_is_disabled() {
        let mut state = enabled_state();
        state.doctrine = DoctrineStatus::Disabled;
        let ops = FakeInstanceOps::new().with_instances(sample_instances());
        let reply = handle_cloud_request("list-instances", "", &state, &ops);
        assert!(!reply.ok);
        assert!(reply.instances.is_none());
        assert!(reply.gated.expect("gated").contains("disabled"));
        assert!(ops.calls().is_empty(), "a gated verb never touches the seam");
    }

    #[test]
    fn list_instances_gates_when_nova_is_not_running() {
        let mut state = enabled_state();
        // nova_api present but exited — the CLI would fail, so gate first.
        state.services[1].status = ServiceStatus::NotRunning {
            podman_state: "exited".into(),
        };
        let ops = FakeInstanceOps::new().with_instances(sample_instances());
        let reply = handle_cloud_request("list-instances", "", &state, &ops);
        assert!(!reply.ok);
        assert!(reply.gated.expect("gated").contains("nova_api"));
        assert!(ops.calls().is_empty());
    }

    // ── lifecycle verbs perform against the real seam ──

    #[test]
    fn lifecycle_verbs_perform_the_op_when_the_cloud_is_up() {
        let state = enabled_state();
        for (verb, want) in [
            ("instance-start", "start:i-9"),
            ("instance-stop", "stop:i-9"),
            ("instance-reboot", "reboot:i-9"),
            ("instance-delete", "delete:i-9"),
        ] {
            let ops = FakeInstanceOps::new();
            let reply = handle_cloud_request(verb, r#"{"instance":"i-9"}"#, &state, &ops);
            assert!(reply.ok, "{verb}");
            assert_eq!(reply.instance.as_deref(), Some("i-9"), "{verb}");
            assert_eq!(ops.calls(), vec![want.to_string()], "{verb}");
        }
    }

    #[test]
    fn destructive_ops_are_flagged_audited_and_reads_are_not() {
        let state = enabled_state();
        // delete + reboot are destructive → audited.
        for verb in ["instance-delete", "instance-reboot"] {
            let ops = FakeInstanceOps::new();
            let reply = handle_cloud_request(verb, r#"{"instance":"i-9"}"#, &state, &ops);
            assert!(reply.ok && reply.audited, "{verb} must be audited");
        }
        // start + stop are not destructive.
        for verb in ["instance-start", "instance-stop"] {
            let ops = FakeInstanceOps::new();
            let reply = handle_cloud_request(verb, r#"{"instance":"i-9"}"#, &state, &ops);
            assert!(reply.ok && !reply.audited, "{verb} is not destructive");
        }
    }

    #[test]
    fn a_destructive_verb_is_gated_and_never_performed_when_disabled() {
        // §7 — delete is performed ONLY when enabled + running; a disabled
        // doctrine gates it, and the seam is never touched (no eviction).
        let mut state = enabled_state();
        state.doctrine = DoctrineStatus::Disabled;
        let ops = FakeInstanceOps::new();
        let reply = handle_cloud_request("instance-delete", r#"{"instance":"i-9"}"#, &state, &ops);
        assert!(!reply.ok);
        assert!(reply.gated.is_some());
        assert!(!reply.audited);
        assert!(ops.calls().is_empty(), "a gated delete never reaches the seam");
    }

    #[test]
    fn a_lifecycle_verb_without_an_instance_is_rejected() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        for body in ["", "{}", r#"{"instance":"  "}"#] {
            let reply = handle_cloud_request("instance-stop", body, &state, &ops);
            assert!(!reply.ok, "body {body:?}");
            assert!(reply.error.expect("error").contains("instance"));
            assert!(ops.calls().is_empty(), "no id ⇒ no seam call");
        }
    }

    #[test]
    fn a_malformed_lifecycle_body_is_a_typed_rejection_not_a_panic() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let reply = handle_cloud_request("instance-start", "[1,2,3]", &state, &ops);
        assert!(!reply.ok);
        assert!(reply.error.expect("error").contains("bad cloud request"));
        assert!(ops.calls().is_empty());
    }

    #[test]
    fn a_seam_failure_is_a_typed_failed_reply_not_a_fake_success() {
        // §7 — the op reached the CLI and it errored; the reply carries the
        // failure, never a fabricated ok.
        let state = enabled_state();
        let ops = FakeInstanceOps::new().failing("exit 1: No server with a name or ID of 'i-9'");
        let reply = handle_cloud_request("instance-start", r#"{"instance":"i-9"}"#, &state, &ops);
        assert!(!reply.ok);
        assert!(reply.gated.is_none(), "a real failure is not a gate");
        assert!(reply.error.expect("error").contains("No server"));
        assert_eq!(ops.calls(), vec!["start:i-9".to_string()], "it did reach the seam");
    }

    #[test]
    fn an_unknown_verb_is_rejected() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let reply = handle_cloud_request("instance-teleport", "{}", &state, &ops);
        assert!(!reply.ok);
        assert!(reply.error.expect("error").contains("unknown cloud verb"));
    }

    // ── typed round-trips ──

    #[test]
    fn reply_round_trips_json_and_carries_the_ok_flag() {
        let reply = CloudReply::instances("list-instances", sample_instances());
        let body = reply.to_body();
        assert!(body.contains(r#""ok":true"#));
        let back: CloudReply = serde_json::from_str(&body).expect("decode");
        assert!(back.ok);
        assert_eq!(back.instances.expect("instances").len(), 1);
        // A gate encodes ok:false + a gated reason, no payload.
        let g = CloudReply::gated("list-instances", "nova down").to_body();
        assert!(g.contains(r#""ok":false"#));
        assert!(g.contains("nova down"));
        assert!(!g.contains(r#""instances""#));
    }

    // ── the openstack CLI argv + parse (pure) ──

    #[test]
    fn openstack_argv_shapes() {
        assert_eq!(
            build_server_list_argv(),
            vec!["server", "list", "-f", "json"]
        );
        assert_eq!(
            build_lifecycle_argv(LifecycleAction::Delete, "i-1"),
            vec!["server", "delete", "i-1"]
        );
        assert_eq!(
            build_lifecycle_argv(LifecycleAction::Reboot, "i-2"),
            vec!["server", "reboot", "i-2"]
        );
    }

    #[test]
    fn server_list_json_parses_the_openstack_columns() {
        // The capitalized-column JSON `openstack server list -f json` emits,
        // with Networks as a string and a missing optional column.
        let body = r#"[
            {"ID":"i-1","Name":"web","Status":"ACTIVE","Flavor":"m1.small","Image":"","Networks":"flat=10.0.0.5"},
            {"ID":"i-2","Name":"db","Status":"SHUTOFF","Flavor":"m1.large","Networks":{"flat":["10.0.0.6"]}}
        ]"#;
        let list = parse_server_list_json(body).expect("parse");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "i-1");
        assert_eq!(list[0].flavor.as_deref(), Some("m1.small"));
        assert!(list[0].image.is_none(), "an empty column stays None");
        assert_eq!(list[0].networks.as_deref(), Some("flat=10.0.0.5"));
        assert_eq!(list[1].status, "SHUTOFF");
        assert!(
            list[1].networks.as_deref().expect("networks").contains("10.0.0.6"),
            "an object Networks column renders to a string"
        );
    }

    #[test]
    fn malformed_server_list_is_a_typed_parse_error() {
        let err = parse_server_list_json("not json").expect_err("must reject");
        assert!(matches!(err, InstanceOpError::Parse(_)));
    }

    // ── the responder drain over a real Persist ──

    #[test]
    fn drain_answers_a_lifecycle_request_on_the_reply_lane() {
        // End-to-end: a client publishes `action/cloud/instance-stop`, the drain
        // performs it against the seam and lands a typed reply on reply/<ulid>.
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let topic = cloud_action_topic("instance-stop");
        let req = persist
            .write(
                &topic,
                Priority::Default,
                None,
                Some(r#"{"instance":"i-42"}"#),
            )
            .expect("write request");

        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let mut cursors = BTreeMap::new();
        drain_cloud_verbs(&persist, &mut cursors, &state, &ops);

        // The op reached the seam...
        assert_eq!(ops.calls(), vec!["stop:i-42".to_string()]);
        // ...and the typed reply landed on reply/<ulid>.
        let replies = persist
            .list_since(&reply_topic(&req.ulid), None)
            .expect("list replies");
        assert_eq!(replies.len(), 1);
        let reply: CloudReply =
            serde_json::from_str(&replies[0].body.clone().expect("reply body")).expect("decode");
        assert!(reply.ok);
        assert_eq!(reply.instance.as_deref(), Some("i-42"));

        // A second drain with the advanced cursor re-answers nothing (no
        // re-performed lifecycle op — the §7 idempotence of the cursor).
        drain_cloud_verbs(&persist, &mut cursors, &state, &ops);
        assert_eq!(ops.calls(), vec!["stop:i-42".to_string()], "no replay");
    }
}
