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

use mackes_mesh_types::openstack::{
    default_collection, EndpointInterface, HealthState, HeatPreview, HeatStackDetail,
    ResourceTable, ServiceCatalog, ServiceHealth,
};

use crate::workers::proc::output_with_timeout;

use super::catalog::ServiceKind;
use super::client::{CatalogHealth, CloudClient};
use super::reconcile::{DoctrineStatus, OpenStackState, RuntimeStatus, ServiceRow, ServiceStatus};

// ─────────────────────────── the verb vocabulary ───────────────────────────

/// The `action/cloud/` namespace prefix every cloud verb request rides.
pub const CLOUD_ACTION_PREFIX: &str = "action/cloud/";

/// Every typed cloud verb, in the order the responder drains them.
///
/// The read verbs carry a `get-`/`list-` stem so they are audit-exempt
/// ([`mde_bus::persist::is_auditable`], the BUS-AUDIT-FLOOD guard); the four
/// `instance-*` lifecycle verbs are control-plane mutations and audit.
///
/// IAC-1 added `get-catalog` — the Keystone service directory + per-service API
/// health the `IaC` surface (IAC-2) consumes; a read, so audit-exempt. IAC-3
/// added `list-resources` — one cataloged service's resource rows the Resources
/// tab renders; also a read (`list-` stem), so audit-exempt.
///
/// IAC-4 added the native **Heat** control loop. The three **reads** carry the
/// `get-` prefix (like `get-catalog`) so they are audit-exempt under
/// [`mde_bus::persist::is_auditable`] (the BUS-AUDIT-FLOOD guard), matching the
/// task's "read verbs audit-exempt":
///
/// - `get-heat-detail` — a stack's resources/events/outputs/template (show),
/// - `get-heat-preview` — a dry-run preview-update diff (no state change),
/// - `get-heat-reverse` — a reverse-generated HOT from live infra.
///
/// The four **mutations** change the cloud and audit (both the auditable Bus
/// topic + a tracing audit line): `heat-check` (drift stack-check), `heat-create`,
/// `heat-update`, `heat-delete` (the last three typed-armed at the surface, #22).
pub const CLOUD_VERBS: [&str; 16] = [
    "get-status",
    "get-catalog",
    "list-services",
    "list-resources",
    "list-instances",
    "instance-start",
    "instance-stop",
    "instance-reboot",
    "instance-delete",
    "get-heat-detail",
    "get-heat-preview",
    "get-heat-reverse",
    "heat-check",
    "heat-create",
    "heat-update",
    "heat-delete",
];

/// Whether a Heat verb is an audited mutation vs an audit-exempt read.
///
/// The mutations `heat-check`/`heat-create`/`heat-update`/`heat-delete` change
/// the cloud; the reads `get-heat-detail`/`get-heat-preview`/`get-heat-reverse`
/// do not. Drives the `audited` reply flag + the tracing audit line; the Bus
/// topic's own hash-chain audit follows [`mde_bus::persist::is_auditable`] (the
/// read verbs carry the `get-` exempt stem).
#[must_use]
pub fn heat_verb_audits(verb: &str) -> bool {
    matches!(
        verb,
        "heat-check" | "heat-create" | "heat-update" | "heat-delete"
    )
}

/// Whether a cloud verb is an **audited mutation** (vs an audit-exempt read) —
/// IAC-5 (#23): every mutating op audits.
///
/// The four `instance-*` lifecycle verbs (a start/stop/reboot/delete all change
/// the cloud) and the four Heat mutations ([`heat_verb_audits`]) audit; the
/// `get-`/`list-` reads do not. This is the single authority the drain uses to
/// (a) set the reply's `audited` flag + emit the tracing audit line, and (b)
/// decide a notify-on-failure — and it agrees, verb-for-verb, with the Bus
/// topic's own hash-chain guard ([`mde_bus::persist::is_auditable`]): every verb
/// for which this is `true` rides an auditable `action/cloud/<verb>` topic, and
/// every read for which it is `false` carries the exempt stem (asserted in the
/// tests).
#[must_use]
pub fn verb_audits(verb: &str) -> bool {
    LifecycleAction::from_verb(verb).is_some() || heat_verb_audits(verb)
}

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
    vec!["server".into(), action.cli_verb().into(), instance.into()]
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

/// The typed request body for `list-resources` (IAC-3) — which service's
/// resources to list.
///
/// `service` is the Keystone service **type** (`compute`/`network`/…, required);
/// `collection` is the REST collection path (defaulted from the service type via
/// [`default_collection`] when absent); `query` are list filters applied verbatim
/// (the linked cross-service view passes e.g. `device_id=<instance>`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceListRequest {
    /// The Keystone service type to list resources for.
    pub service: String,
    /// The collection path (`servers/detail`, `stacks`, …); defaulted from the
    /// service type when empty/absent.
    pub collection: Option<String>,
    /// List filters (`[["status","ACTIVE"]]`), applied verbatim.
    pub query: Vec<(String, String)>,
}

/// Parse a `list-resources` request body into a typed [`ResourceListRequest`].
///
/// # Errors
/// A human-readable message when the body isn't valid request JSON.
pub fn parse_resource_request(body: &str) -> Result<ResourceListRequest, String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok(ResourceListRequest::default());
    }
    serde_json::from_str(trimmed).map_err(|e| format!("bad resource request body: {e}"))
}

/// The typed request body for the `heat-*` verbs (IAC-4).
///
/// Which fields are read depends on the verb: `get-heat-detail` needs `stack`
/// (id or name); preview/check/update/delete need `stack_name` + `stack_id`;
/// create needs `stack_name`; preview/create/update carry the `template` buffer;
/// reverse carries the `services` `(type, collection)` list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HeatRequest {
    /// The stack id-or-name for a `get-heat-detail` show.
    pub stack: String,
    /// The stack name (preview / check / update / delete / create).
    pub stack_name: String,
    /// The stack id (preview / check / update / delete).
    pub stack_id: String,
    /// The HOT template buffer (preview / create / update).
    pub template: String,
    /// The `(service_type, collection)` list to reverse-generate from.
    pub services: Vec<(String, String)>,
}

/// Parse a `heat-*` request body into a typed [`HeatRequest`].
///
/// # Errors
/// A human-readable message when the body isn't valid request JSON.
pub fn parse_heat_request(body: &str) -> Result<HeatRequest, String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok(HeatRequest::default());
    }
    serde_json::from_str(trimmed).map_err(|e| format!("bad heat request body: {e}"))
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
    /// `get-catalog` — the Keystone service directory (IAC-1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog: Option<ServiceCatalog>,
    /// `get-catalog` — the per-service API health rows, paired with the catalog
    /// (IAC-1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<Vec<ServiceHealth>>,
    /// `list-resources` — one cataloged service's resource table (IAC-3).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceTable>,
    /// `list-instances` — the Nova instance roster.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instances: Option<Vec<CloudInstance>>,
    /// The instance a lifecycle verb acted on, on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// `get-heat-detail` — a stack's full detail (IAC-4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heat_detail: Option<HeatStackDetail>,
    /// `get-heat-preview` — a preview-update dry-run diff (IAC-4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heat_preview: Option<HeatPreview>,
    /// `get-heat-reverse` — a reverse-generated HOT template (IAC-4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// The stack a Heat mutation acted on / created, on success (IAC-4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
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
            catalog: None,
            health: None,
            resources: None,
            instances: None,
            instance: None,
            heat_detail: None,
            heat_preview: None,
            template: None,
            stack: None,
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

    /// `get-catalog` — the Keystone service directory + per-service API health
    /// (IAC-1; both come from one authenticate + probe pass).
    #[must_use]
    pub fn catalog(verb: &str, ch: CatalogHealth) -> Self {
        Self {
            catalog: Some(ch.catalog),
            health: Some(ch.health),
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

    /// `list-resources` — one cataloged service's resource table (IAC-3).
    #[must_use]
    pub fn resources(verb: &str, table: ResourceTable) -> Self {
        Self {
            resources: Some(table),
            ..Self::base(verb, true)
        }
    }

    /// `get-heat-detail` — one stack's full detail (IAC-4).
    #[must_use]
    pub fn heat_detail(verb: &str, detail: HeatStackDetail) -> Self {
        Self {
            heat_detail: Some(detail),
            ..Self::base(verb, true)
        }
    }

    /// `get-heat-preview` — a preview-update dry-run diff (IAC-4).
    #[must_use]
    pub fn heat_preview(verb: &str, preview: HeatPreview) -> Self {
        Self {
            heat_preview: Some(preview),
            ..Self::base(verb, true)
        }
    }

    /// `get-heat-reverse` — a reverse-generated HOT template (IAC-4).
    #[must_use]
    pub fn heat_template(verb: &str, template: impl Into<String>) -> Self {
        Self {
            template: Some(template.into()),
            ..Self::base(verb, true)
        }
    }

    /// A Heat mutation succeeded on `stack` (`audited` for the change verbs —
    /// check/create/update/delete, IAC-4/#23).
    #[must_use]
    pub fn heat_performed(verb: &str, stack: impl Into<String>, audited: bool) -> Self {
        Self {
            stack: Some(stack.into()),
            audited,
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

    /// A lifecycle op succeeded on `instance` (`audited` — every performed
    /// lifecycle verb is a control-plane mutation and audits, #23).
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
            return Err(format!(
                "the cloud doctrine is unread on {} — {reason}",
                state.host
            ))
        }
    }
    if let RuntimeStatus::Unavailable { reason } = &state.runtime {
        return Err(format!(
            "the container runtime is unavailable on {} — {reason}",
            state.host
        ));
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
    client: &dyn CloudClient,
) -> CloudReply {
    match verb {
        "get-status" => CloudReply::status(verb, state.clone()),
        "list-services" => CloudReply::services(verb, state.services.clone()),
        // IAC-1 — the Keystone service directory + per-service API health. An
        // unconfigured node (no clouds.yaml) is an honest gate; an auth/transport
        // failure is a real failure — never a fabricated catalog (§7).
        "get-catalog" => match client.catalog_and_health() {
            Ok(ch) => CloudReply::catalog(verb, ch),
            Err(e) if e.is_unconfigured() => CloudReply::gated(verb, e.to_string()),
            Err(e) => CloudReply::failed(verb, e.to_string()),
        },
        // IAC-3 — one cataloged service's resource rows (the Resources tab).
        // Same client seam as get-catalog: an unconfigured node gates honestly, an
        // auth/transport/parse failure is a real failure — never fabricated rows
        // (§7). An empty table is a real "no resources", carried as an ok reply.
        "list-resources" => handle_list_resources(verb, body, client),
        // IAC-4 — the native Heat control loop (reads + mutations). Same client
        // seam: an unconfigured node gates, a transport/parse failure is a real
        // failure, never a fabricated stack/diff/template (§7). The mutations
        // (check/create/update/delete) audit; the reads do not.
        "get-heat-detail" | "get-heat-preview" | "get-heat-reverse" | "heat-check"
        | "heat-create" | "heat-update" | "heat-delete" => handle_heat(verb, body, client),
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
            // #23 — every performed lifecycle verb is a control-plane mutation and
            // audits (start/stop change instance state just as reboot/delete do):
            // the request topic hash-chained via `is_auditable`, and we log the
            // performed op on `mackesd::openstack`. `destructive` is a separate
            // axis (it drives the surface's typed-arming, not the audit).
            tracing::info!(
                target: "mackesd::openstack",
                verb,
                instance,
                destructive = action.is_destructive(),
                "performed cloud lifecycle mutation (audited)"
            );
            CloudReply::performed(verb, instance, true)
        }
        Err(e) => CloudReply::failed(verb, e.to_string()),
    }
}

/// The `list-resources` leg of [`handle_cloud_request`]: parse the target
/// service, default its collection from the service type, drive the client's
/// resource-list seam, and answer honestly.
///
/// An unconfigured node gates (retry once the cloud is configured); an
/// auth/transport/parse failure is a real failure; a service with no known
/// collection is a typed rejection. An empty table is a real ok reply ("no
/// resources"), never a fabricated row (§7).
fn handle_list_resources(verb: &str, body: &str, client: &dyn CloudClient) -> CloudReply {
    let req = match parse_resource_request(body) {
        Ok(r) => r,
        Err(e) => return CloudReply::rejected(verb, e),
    };
    let service = req.service.trim();
    if service.is_empty() {
        return CloudReply::rejected(
            verb,
            "a list-resources request requires a non-empty `service` (the Keystone service type)",
        );
    }
    let collection = req
        .collection
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string)
        .or_else(|| default_collection(service).map(str::to_string));
    let Some(collection) = collection else {
        return CloudReply::rejected(
            verb,
            format!("no default resource collection for service `{service}` — pass an explicit `collection`"),
        );
    };
    match client.list_resources(service, &collection, &req.query) {
        Ok(table) => CloudReply::resources(verb, table),
        Err(e) if e.is_unconfigured() => CloudReply::gated(verb, e.to_string()),
        Err(e) => CloudReply::failed(verb, e.to_string()),
    }
}

/// The Heat leg of [`handle_cloud_request`] (IAC-4): parse the typed request,
/// drive the client's Heat seam for the verb, and answer honestly.
///
/// Every verb maps an [`ClientError::Unconfigured`] to a gate (retry once the
/// cloud is configured) and any other error to a real failure — never a
/// fabricated stack/diff/template (§7). A performed mutation
/// (check/create/update/delete) is logged on `mackesd::openstack` and its reply
/// carries `audited: true` (#23); the request topic itself hash-chains via
/// [`mde_bus::persist::is_auditable`]. A malformed body / a mutation missing its
/// stack ref is a typed rejection.
fn handle_heat(verb: &str, body: &str, client: &dyn CloudClient) -> CloudReply {
    let req = match parse_heat_request(body) {
        Ok(r) => r,
        Err(e) => return CloudReply::rejected(verb, e),
    };
    match verb {
        "get-heat-detail" => {
            let stack = req.stack.trim();
            if stack.is_empty() {
                return CloudReply::rejected(
                    verb,
                    "a get-heat-detail request requires a non-empty `stack` (id or name)",
                );
            }
            heat_result(verb, client.heat_show(stack), |detail| {
                CloudReply::heat_detail(verb, detail)
            })
        }
        "get-heat-preview" => {
            let Some((name, id)) = require_stack_ref(&req) else {
                return reject_missing_stack_ref(verb);
            };
            heat_result(
                verb,
                client.heat_preview(name, id, &req.template),
                |preview| CloudReply::heat_preview(verb, preview),
            )
        }
        "get-heat-reverse" => heat_result(verb, client.heat_reverse(&req.services), |hot| {
            CloudReply::heat_template(verb, hot)
        }),
        "heat-check" => {
            let Some((name, id)) = require_stack_ref(&req) else {
                return reject_missing_stack_ref(verb);
            };
            heat_mutation(verb, &format!("{name}/{id}"), client.heat_check(name, id))
        }
        "heat-create" => {
            let name = req.stack_name.trim();
            if name.is_empty() {
                return CloudReply::rejected(
                    verb,
                    "a heat-create request requires a non-empty `stack_name`",
                );
            }
            match client.heat_create(name, &req.template) {
                Ok(new_id) => {
                    let stack = if new_id.is_empty() { name } else { &new_id };
                    heat_audit_ok(verb, stack, stack.to_string())
                }
                Err(e) if e.is_unconfigured() => CloudReply::gated(verb, e.to_string()),
                Err(e) => CloudReply::failed(verb, e.to_string()),
            }
        }
        "heat-update" => {
            let Some((name, id)) = require_stack_ref(&req) else {
                return reject_missing_stack_ref(verb);
            };
            heat_mutation(
                verb,
                &format!("{name}/{id}"),
                client.heat_update(name, id, &req.template),
            )
        }
        "heat-delete" => {
            let Some((name, id)) = require_stack_ref(&req) else {
                return reject_missing_stack_ref(verb);
            };
            heat_mutation(verb, &format!("{name}/{id}"), client.heat_delete(name, id))
        }
        other => CloudReply::rejected(verb, format!("unknown heat verb: {other}")),
    }
}

/// The `(stack_name, stack_id)` a Heat verb targets, or `None` when either is
/// empty (the canonical Heat URL needs both).
fn require_stack_ref(req: &HeatRequest) -> Option<(&str, &str)> {
    let name = req.stack_name.trim();
    let id = req.stack_id.trim();
    (!name.is_empty() && !id.is_empty()).then_some((name, id))
}

/// The typed rejection for a Heat verb whose `stack_name` + `stack_id` isn't
/// fully specified.
fn reject_missing_stack_ref(verb: &str) -> CloudReply {
    CloudReply::rejected(
        verb,
        "this heat verb requires a non-empty `stack_name` + `stack_id`",
    )
}

/// Fold a Heat read result into a reply: `ok` → the payload reply, an
/// unconfigured error → a gate, any other error → a failure.
fn heat_result<T>(
    verb: &str,
    result: Result<T, super::client::ClientError>,
    ok: impl FnOnce(T) -> CloudReply,
) -> CloudReply {
    match result {
        Ok(v) => ok(v),
        Err(e) if e.is_unconfigured() => CloudReply::gated(verb, e.to_string()),
        Err(e) => CloudReply::failed(verb, e.to_string()),
    }
}

/// Fold a Heat mutation result (check/update/delete) into a reply, auditing a
/// performed op (#23).
fn heat_mutation(
    verb: &str,
    stack: &str,
    result: Result<(), super::client::ClientError>,
) -> CloudReply {
    match result {
        Ok(()) => heat_audit_ok(verb, stack, stack.to_string()),
        Err(e) if e.is_unconfigured() => CloudReply::gated(verb, e.to_string()),
        Err(e) => CloudReply::failed(verb, e.to_string()),
    }
}

/// Log a performed Heat mutation on `mackesd::openstack` (audited, #23) and build
/// the ok reply. `audited` follows [`heat_verb_audits`] (every mutation audits).
fn heat_audit_ok(verb: &str, stack_ref: &str, reply_stack: String) -> CloudReply {
    let audited = heat_verb_audits(verb);
    if audited {
        tracing::info!(
            target: "mackesd::openstack",
            verb,
            stack = stack_ref,
            "performed Heat mutation (audited)"
        );
    }
    CloudReply::heat_performed(verb, reply_stack, audited)
}

// ─────────────────────────── the mesh notify feed (IAC-5) ───────────────────────────

/// CHAT-FIX-2 — the mesh notify lane the `openstack` worker fires on. The `chat`
/// worker folds every `event/notify/*` lane
/// ([`crate::workers::chat::ALERT_LANE_PREFIXES`]) into the node's `alert:<self>`
/// conversation (and, Warning+, the tray badge / a chyron), so a body published
/// here reaches the operator's Chat feed with **no** emitter-side render path —
/// the same glue [`crate::workers::notify`] + [`crate::workers::node_grade`] use
/// (§6, reuse-not-duplicate).
const CLOUD_NOTIFY_TOPIC: &str = "event/notify/cloud";

/// The stable `source` token on the published alert body (the Chat card badge).
const CLOUD_NOTIFY_SOURCE: &str = "cloud";

/// Severity tag ([`mde_chat::Severity`] shape, folded by `fold_alert`) for a
/// mutating-op **failure** — worth noticing (amber).
const SEV_WARNING: &str = "warning";

/// Severity tag for a **service going down** — needs attention now (red).
const SEV_CRITICAL: &str = "critical";

/// The alert-shaped JSON body a cloud notification serializes to — the shape the
/// chat [`mde_chat::fold_alert`] understands (`severity` drives the colour;
/// `host` routes it to `alert:<host>`; every other string field becomes a card
/// row). Mirrors [`crate::workers::node_grade`]'s `GradeAlertBody`.
#[derive(Debug, Serialize)]
struct CloudAlertBody<'a> {
    /// `warning` (op failure) / `critical` (service down).
    severity: &'a str,
    /// The `cloud` source badge.
    source: &'a str,
    /// The one-line human message the Chat card shows.
    summary: String,
    /// The originating host — routes the alert to `alert:<host>`.
    host: &'a str,
    /// The cloud verb that failed, for an op-failure alert.
    #[serde(skip_serializing_if = "Option::is_none")]
    verb: Option<&'a str>,
    /// The service that went down, for a service-down alert.
    #[serde(skip_serializing_if = "Option::is_none")]
    service: Option<&'a str>,
    /// The alert's mint time (ms since epoch).
    ts_unix_ms: i64,
}

/// The `openstack` worker's mesh-notify producer (IAC-5 / #23).
///
/// It fires the notify feed **only** on a mutating-op failure or a health-probe
/// service-down transition — never a routine success (that would be feed noise).
/// It carries the small cross-tick state the second trigger needs: this node's
/// host stamp (the alert's origin) and the last-seen per-service health (to catch
/// an `Up → Down` edge, seeded silently on first sight so a boot doesn't flood).
/// It rides in/out of [`drain_cloud_verbs`] by value alongside the verb cursors.
#[derive(Debug, Default, Clone)]
pub struct CloudNotifier {
    /// This node's id — the alert's `host` (routes it to `alert:<host>`).
    host: String,
    /// Last-seen per-service health state (public interface preferred), keyed by
    /// service type — the baseline the `Up → Down` edge is measured against.
    seen_health: BTreeMap<String, HealthState>,
}

impl CloudNotifier {
    /// A notifier stamping alerts with this node's `host`.
    #[must_use]
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            seen_health: BTreeMap::new(),
        }
    }

    /// Publish an op-failure alert for a mutating verb whose reply carried an
    /// error (a seam failure or a rejection) — a gate (`retry later`) or a routine
    /// success never reaches here (see [`mutation_failed`]).
    fn emit_op_failure(&self, persist: &Persist, verb: &str, reply: &CloudReply, now_ms: i64) {
        let detail = reply.error.as_deref().unwrap_or("unknown error");
        let summary = format!("OpenStack {verb} failed on {}: {detail}", self.host);
        Self::publish(
            persist,
            &CloudAlertBody {
                severity: SEV_WARNING,
                source: CLOUD_NOTIFY_SOURCE,
                summary,
                host: &self.host,
                verb: Some(verb),
                service: None,
                ts_unix_ms: now_ms,
            },
        );
    }

    /// Fold a fresh `get-catalog` health snapshot: fire a service-down alert for
    /// every service that transitioned `Up → Down` since the last snapshot, then
    /// re-baseline. First sight (no prior state) seeds silently — a service that
    /// is simply down on the first poll is not a *transition* (§7 — no boot flood).
    fn note_catalog_health(&mut self, persist: &Persist, health: &[ServiceHealth], now_ms: i64) {
        let now = service_states(health);
        for ty in down_transitions(&self.seen_health, &now) {
            let summary = format!("OpenStack {ty} service went down on {}", self.host);
            Self::publish(
                persist,
                &CloudAlertBody {
                    severity: SEV_CRITICAL,
                    source: CLOUD_NOTIFY_SOURCE,
                    summary,
                    host: &self.host,
                    verb: None,
                    service: Some(&ty),
                    ts_unix_ms: now_ms,
                },
            );
        }
        self.seen_health = now;
    }

    /// Serialize + publish one alert body on [`CLOUD_NOTIFY_TOPIC`]. Best-effort —
    /// a write failure is logged, never fatal (§7).
    fn publish(persist: &Persist, body: &CloudAlertBody) {
        let Ok(json) = serde_json::to_string(body) else {
            return;
        };
        if let Err(e) = persist.write(CLOUD_NOTIFY_TOPIC, Priority::Default, None, Some(&json)) {
            tracing::debug!(
                target: "mackesd::openstack",
                topic = CLOUD_NOTIFY_TOPIC,
                error = %e,
                "cloud notify publish failed"
            );
        }
    }
}

/// Whether a reply warrants an op-failure notify: a **mutating** verb
/// ([`verb_audits`]) whose reply carried an `error` (a seam failure or a
/// rejection). A read never notifies; a routine success (`error` unset) and an
/// honest gate (`gated` set, `error` unset — retry later, nothing performed) do
/// not either (#23 — only on failure).
#[must_use]
fn mutation_failed(verb: &str, reply: &CloudReply) -> bool {
    verb_audits(verb) && reply.error.is_some()
}

/// Reduce per-endpoint health rows to one state per service type — the **public**
/// interface's state when the service advertises one (what a mesh client
/// reaches), else the first probed interface's. The single representative the
/// `Up → Down` edge is measured on.
fn service_states(health: &[ServiceHealth]) -> BTreeMap<String, HealthState> {
    let mut out: BTreeMap<String, HealthState> = BTreeMap::new();
    for h in health {
        let is_public = h.interface == EndpointInterface::Public;
        // Public wins; otherwise keep the first interface seen for this type.
        if is_public || !out.contains_key(&h.service_type) {
            out.insert(h.service_type.clone(), h.state);
        }
    }
    out
}

/// The service types that transitioned `Up → Down` between two health snapshots —
/// the services worth a "went down" alert. Only a prior **Up** counts as a
/// transition: a first sight (absent from `prev`), a still-down service, and an
/// `Absent → Down` (an endpoint that was never up) are all silent (§7).
fn down_transitions(
    prev: &BTreeMap<String, HealthState>,
    now: &BTreeMap<String, HealthState>,
) -> Vec<String> {
    now.iter()
        .filter(|(ty, &state)| {
            state == HealthState::Down && prev.get(*ty) == Some(&HealthState::Up)
        })
        .map(|(ty, _)| ty.clone())
        .collect()
}

/// Milliseconds since the Unix epoch — the alert body's `ts_unix_ms` stamp.
fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// Drain net-new `action/cloud/*` requests and answer each on `reply/<ulid>`.
///
/// Answers each net-new request since the per-verb cursors with a typed
/// [`CloudReply`] over `state` + the `ops` seam + the `client` seam. Best-effort
/// — a read/write failure is logged and the cursor still advances (a stale
/// request never re-answers, and a lifecycle op never re-performs).
///
/// Synchronous + seam-pure: the worker drives it on a blocking task (an
/// `openstack` shell-out never pins the async runtime); tests drive it directly
/// with a real [`Persist`] tempdir + [`super::testkit::FakeInstanceOps`].
///
/// IAC-5 (#23) — after each request settles, `notifier` fires the mesh notify
/// feed **only** on a mutating-op failure ([`mutation_failed`]) or a
/// `get-catalog` health-probe `Up → Down` transition; a routine success or an
/// honest gate never publishes a notification.
pub fn drain_cloud_verbs(
    persist: &Persist,
    cursors: &mut BTreeMap<String, Option<String>>,
    state: &OpenStackState,
    ops: &dyn InstanceOps,
    client: &dyn CloudClient,
    notifier: &mut CloudNotifier,
) {
    let now_ms = now_unix_ms();
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
            let reply = handle_cloud_request(verb, &body, state, ops, client);
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply.to_body()),
            ) {
                tracing::warn!(target: "mackesd::openstack", ulid = %msg.ulid, error = %e, "cloud verb: reply write failed");
            }
            // IAC-5 — fire the mesh notify feed only on a mutating-op failure or a
            // service-down transition (#23), never a routine success.
            if mutation_failed(verb, &reply) {
                notifier.emit_op_failure(persist, verb, &reply, now_ms);
            } else if verb == "get-catalog" && reply.ok {
                if let Some(health) = reply.health.as_deref() {
                    notifier.note_catalog_health(persist, health, now_ms);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::client::testkit::FakeCatalogSource;
    use super::super::testkit::FakeInstanceOps;
    use super::*;
    use mackes_mesh_types::openstack::{
        shape_health, EndpointInterface, HealthState, ProbeOutcome,
    };

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
        for read in [
            "get-status",
            "get-catalog",
            "list-services",
            "list-resources",
            "list-instances",
        ] {
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
        // IAC-4 — the Heat reads carry the `get-` exempt stem; the mutations audit.
        for read in ["get-heat-detail", "get-heat-preview", "get-heat-reverse"] {
            assert!(
                !mde_bus::persist::is_auditable(&cloud_action_topic(read)),
                "heat read {read} must be audit-exempt (BUS-AUDIT-FLOOD)"
            );
            assert!(!heat_verb_audits(read), "{read} is a read");
        }
        for mutation in ["heat-check", "heat-create", "heat-update", "heat-delete"] {
            assert!(
                mde_bus::persist::is_auditable(&cloud_action_topic(mutation)),
                "heat mutation {mutation} must audit"
            );
            assert!(heat_verb_audits(mutation), "{mutation} is a mutation");
        }
    }

    // ── reads fold the real state model ──

    #[test]
    fn get_status_returns_the_whole_mirror() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let reply = handle_cloud_request(
            "get-status",
            "",
            &state,
            &ops,
            &FakeCatalogSource::unconfigured(),
        );
        assert!(reply.ok);
        assert_eq!(reply.status.expect("status").host, "node-a");
        assert!(ops.calls().is_empty(), "a read never touches the seam");
    }

    #[test]
    fn list_services_returns_the_service_rows() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let reply = handle_cloud_request(
            "list-services",
            "{}",
            &state,
            &ops,
            &FakeCatalogSource::unconfigured(),
        );
        assert!(reply.ok);
        let rows = reply.services.expect("services");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.service == "nova_api"));
    }

    // ── IAC-1: get-catalog drives the client seam / honest-degrades ──

    fn sample_catalog_health() -> CatalogHealth {
        let catalog = ServiceCatalog::from_keystone_token_json(
            r#"{"token":{"catalog":[
                {"type":"compute","name":"nova","endpoints":[
                    {"interface":"public","url":"http://nova.mesh:8774/v2.1","region":"RegionOne"}
                ]}
            ]}}"#,
        )
        .unwrap();
        let health = vec![shape_health(
            "compute",
            EndpointInterface::Public,
            "http://nova.mesh:8774/v2.1",
            &ProbeOutcome::Reachable {
                http_status: 200,
                body: r#"{"version":{"id":"v2.1","max_version":"2.90"}}"#.into(),
                elapsed_ms: 5,
            },
        )];
        CatalogHealth { catalog, health }
    }

    #[test]
    fn get_catalog_returns_the_catalog_and_health_from_the_client() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let ch = sample_catalog_health();
        let cat = FakeCatalogSource::ok(ch.catalog, ch.health);
        let reply = handle_cloud_request("get-catalog", "", &state, &ops, &cat);
        assert!(reply.ok);
        let catalog = reply.catalog.expect("catalog");
        assert_eq!(catalog.services.len(), 1);
        assert_eq!(catalog.services[0].service_type, "compute");
        let health = reply.health.expect("health");
        assert_eq!(health[0].state, HealthState::Up);
        assert_eq!(health[0].microversion.as_deref(), Some("2.90"));
        assert!(
            ops.calls().is_empty(),
            "get-catalog never touches the instance seam"
        );
    }

    #[test]
    fn get_catalog_gates_honestly_when_no_cloud_is_configured() {
        // §7 — a node with no clouds.yaml gates (retry once configured), never a
        // fabricated empty catalog.
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let reply = handle_cloud_request(
            "get-catalog",
            "",
            &state,
            &ops,
            &FakeCatalogSource::unconfigured(),
        );
        assert!(!reply.ok);
        assert!(reply.catalog.is_none() && reply.health.is_none());
        assert!(reply.gated.expect("gated").contains("no clouds.yaml"));
    }

    #[test]
    fn get_catalog_reports_a_real_auth_failure_as_failed_not_gated() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let cat = FakeCatalogSource::failing(crate::workers::openstack::client::ClientError::Auth(
            "401 Unauthorized".into(),
        ));
        let reply = handle_cloud_request("get-catalog", "", &state, &ops, &cat);
        assert!(!reply.ok);
        assert!(reply.gated.is_none(), "a real auth failure is not a gate");
        assert!(reply.error.expect("error").contains("401"));
    }

    // ── IAC-3: list-resources drives the client seam / honest-degrades ──

    fn sample_resource_table() -> ResourceTable {
        ResourceTable::from_collection_json(
            "compute",
            "servers/detail",
            r#"{"servers":[
                {"id":"i-1","name":"web","status":"ACTIVE"},
                {"id":"i-2","name":"db","status":"SHUTOFF"}
            ]}"#,
        )
        .expect("fixture table")
    }

    #[test]
    fn list_resources_returns_a_table_from_the_client() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let cat = FakeCatalogSource::ok(ServiceCatalog::default(), vec![])
            .with_resources(sample_resource_table());
        // The service alone is enough — the collection defaults from the type.
        let reply = handle_cloud_request(
            "list-resources",
            r#"{"service":"compute"}"#,
            &state,
            &ops,
            &cat,
        );
        assert!(reply.ok);
        let table = reply.resources.expect("resources");
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0].id, "i-1");
        assert!(
            ops.calls().is_empty(),
            "list-resources never touches the instance CLI seam"
        );
    }

    #[test]
    fn list_resources_gates_honestly_when_unconfigured_and_fails_on_a_real_error() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        // No clouds.yaml → a gate (retry once configured), never fabricated rows.
        let gated = handle_cloud_request(
            "list-resources",
            r#"{"service":"network"}"#,
            &state,
            &ops,
            &FakeCatalogSource::unconfigured(),
        );
        assert!(!gated.ok && gated.resources.is_none());
        assert!(gated.gated.expect("gated").contains("clouds.yaml"));
        // A real transport failure is failed, not gated.
        let failing = FakeCatalogSource::failing(
            crate::workers::openstack::client::ClientError::Transport("HTTP 500".into()),
        );
        let reply = handle_cloud_request(
            "list-resources",
            r#"{"service":"network"}"#,
            &state,
            &ops,
            &failing,
        );
        assert!(!reply.ok && reply.gated.is_none());
        assert!(reply.error.expect("error").contains("500"));
    }

    #[test]
    fn list_resources_rejects_a_missing_service_or_an_uncollectable_one() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let cat = FakeCatalogSource::ok(ServiceCatalog::default(), vec![]);
        // No service → rejected.
        let bad = handle_cloud_request("list-resources", "{}", &state, &ops, &cat);
        assert!(!bad.ok);
        assert!(bad.error.expect("error").contains("service"));
        // A service with no default collection + no explicit one → rejected
        // (never a fabricated table).
        let no_coll = handle_cloud_request(
            "list-resources",
            r#"{"service":"identity"}"#,
            &state,
            &ops,
            &cat,
        );
        assert!(!no_coll.ok);
        assert!(no_coll.error.expect("error").contains("collection"));
    }

    // ── IAC-4: the Heat control loop drives the client seam / honest-degrades ──

    fn sample_heat_detail() -> HeatStackDetail {
        HeatStackDetail::from_stack_json(
            r#"{"stack":{"id":"s-1","stack_name":"mesh-net","stack_status":"CREATE_COMPLETE"}}"#,
        )
        .unwrap()
        .with_resources_json(
            r#"{"resources":[{"resource_name":"net","resource_type":"OS::Neutron::Net","resource_status":"CREATE_COMPLETE"}]}"#,
        )
    }

    #[test]
    fn heat_get_detail_returns_the_detail_from_the_client() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let cat = FakeCatalogSource::ok(ServiceCatalog::default(), vec![])
            .with_heat_detail(sample_heat_detail());
        let reply = handle_cloud_request(
            "get-heat-detail",
            r#"{"stack":"mesh-net"}"#,
            &state,
            &ops,
            &cat,
        );
        assert!(reply.ok);
        let detail = reply.heat_detail.expect("detail");
        assert_eq!(detail.stack_name, "mesh-net");
        assert_eq!(detail.resources.len(), 1);
        assert!(!reply.audited, "a read is never audited");
        assert_eq!(cat.heat_calls(), vec!["show:mesh-net".to_string()]);
    }

    #[test]
    fn heat_get_detail_rejects_an_empty_stack_and_gates_when_unconfigured() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        // No stack ref → typed rejection (never a fabricated stack).
        let bad = handle_cloud_request(
            "get-heat-detail",
            "{}",
            &state,
            &ops,
            &FakeCatalogSource::ok(ServiceCatalog::default(), vec![]),
        );
        assert!(!bad.ok && bad.error.expect("error").contains("stack"));
        // Unconfigured node → a gate, not a failure.
        let gated = handle_cloud_request(
            "get-heat-detail",
            r#"{"stack":"x"}"#,
            &state,
            &ops,
            &FakeCatalogSource::unconfigured(),
        );
        assert!(!gated.ok && gated.gated.expect("gated").contains("clouds.yaml"));
    }

    #[test]
    fn heat_get_preview_returns_a_diff_and_needs_a_stack_ref() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let preview = HeatPreview {
            added: vec!["new".into()],
            ..HeatPreview::default()
        };
        let cat =
            FakeCatalogSource::ok(ServiceCatalog::default(), vec![]).with_heat_preview(preview);
        let reply = handle_cloud_request(
            "get-heat-preview",
            r#"{"stack_name":"mesh-net","stack_id":"s-1","template":"heat_template_version: 2021-04-16"}"#,
            &state,
            &ops,
            &cat,
        );
        assert!(reply.ok);
        assert_eq!(reply.heat_preview.expect("preview").change_count(), 1);
        assert!(!reply.audited, "a dry-run preview is not audited");
        assert_eq!(cat.heat_calls(), vec!["preview:mesh-net/s-1".to_string()]);
        // Missing stack ref → rejection.
        let bad = handle_cloud_request(
            "get-heat-preview",
            r#"{"stack_name":"mesh-net"}"#,
            &state,
            &ops,
            &FakeCatalogSource::ok(ServiceCatalog::default(), vec![]),
        );
        assert!(!bad.ok && bad.error.expect("error").contains("stack_id"));
    }

    #[test]
    fn heat_get_reverse_returns_a_generated_hot() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let cat = FakeCatalogSource::ok(ServiceCatalog::default(), vec![])
            .with_heat_reverse("heat_template_version: 2021-04-16\nresources: {}\n");
        let reply = handle_cloud_request(
            "get-heat-reverse",
            r#"{"services":[["compute","servers/detail"]]}"#,
            &state,
            &ops,
            &cat,
        );
        assert!(reply.ok);
        assert!(reply
            .template
            .expect("template")
            .contains("heat_template_version"));
        assert!(!reply.audited);
        assert_eq!(cat.heat_calls(), vec!["reverse:1".to_string()]);
    }

    #[test]
    fn heat_mutations_perform_and_audit_and_are_rejected_without_a_ref() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        for (verb, want) in [
            ("heat-check", "check:mesh-net/s-1"),
            ("heat-update", "update:mesh-net/s-1"),
            ("heat-delete", "delete:mesh-net/s-1"),
        ] {
            let cat = FakeCatalogSource::ok(ServiceCatalog::default(), vec![]);
            let reply = handle_cloud_request(
                verb,
                r#"{"stack_name":"mesh-net","stack_id":"s-1","template":"x"}"#,
                &state,
                &ops,
                &cat,
            );
            assert!(reply.ok, "{verb}");
            assert!(reply.audited, "{verb} is an audited mutation (#23)");
            assert_eq!(reply.stack.as_deref(), Some("mesh-net/s-1"), "{verb}");
            assert_eq!(cat.heat_calls(), vec![want.to_string()], "{verb}");
        }
        // A mutation missing its stack ref is a typed rejection (never a blind op).
        let bad = handle_cloud_request(
            "heat-delete",
            "{}",
            &state,
            &ops,
            &FakeCatalogSource::ok(ServiceCatalog::default(), vec![]),
        );
        assert!(!bad.ok && bad.error.expect("error").contains("stack"));
    }

    #[test]
    fn heat_create_needs_a_name_audits_and_returns_the_new_id() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let cat = FakeCatalogSource::ok(ServiceCatalog::default(), vec![])
            .with_resources(ResourceTable::default());
        // The fake's heat mutation answers an empty id → the reply falls back to
        // the name; drive create with a name + template.
        let reply = handle_cloud_request(
            "heat-create",
            r#"{"stack_name":"fresh","template":"heat_template_version: 2021-04-16"}"#,
            &state,
            &ops,
            &cat,
        );
        assert!(reply.ok && reply.audited);
        assert_eq!(reply.stack.as_deref(), Some("fresh"));
        assert_eq!(cat.heat_calls(), vec!["create:fresh".to_string()]);
        // No name → rejection.
        let bad = handle_cloud_request(
            "heat-create",
            r#"{"template":"x"}"#,
            &state,
            &ops,
            &FakeCatalogSource::ok(ServiceCatalog::default(), vec![]),
        );
        assert!(!bad.ok && bad.error.expect("error").contains("stack_name"));
    }

    #[test]
    fn a_heat_mutation_is_failed_not_gated_on_a_real_error() {
        // §7 — a real transport failure is a failure, not a gate; nothing faked.
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let cat = FakeCatalogSource::failing(
            crate::workers::openstack::client::ClientError::Transport("HTTP 409".into()),
        );
        let reply = handle_cloud_request(
            "heat-delete",
            r#"{"stack_name":"n","stack_id":"i"}"#,
            &state,
            &ops,
            &cat,
        );
        assert!(!reply.ok && reply.gated.is_none());
        assert!(reply.error.expect("error").contains("409"));
    }

    // ── list-instances drives the real seam / honest-gates ──

    #[test]
    fn list_instances_drives_the_seam_when_the_cloud_is_up() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new().with_instances(sample_instances());
        let reply = handle_cloud_request(
            "list-instances",
            "",
            &state,
            &ops,
            &FakeCatalogSource::unconfigured(),
        );
        assert!(reply.ok);
        assert_eq!(reply.instances.expect("instances")[0].id, "i-1");
        assert_eq!(ops.calls(), vec!["list".to_string()]);
    }

    #[test]
    fn list_instances_gates_when_the_doctrine_is_disabled() {
        let mut state = enabled_state();
        state.doctrine = DoctrineStatus::Disabled;
        let ops = FakeInstanceOps::new().with_instances(sample_instances());
        let reply = handle_cloud_request(
            "list-instances",
            "",
            &state,
            &ops,
            &FakeCatalogSource::unconfigured(),
        );
        assert!(!reply.ok);
        assert!(reply.instances.is_none());
        assert!(reply.gated.expect("gated").contains("disabled"));
        assert!(
            ops.calls().is_empty(),
            "a gated verb never touches the seam"
        );
    }

    #[test]
    fn list_instances_gates_when_nova_is_not_running() {
        let mut state = enabled_state();
        // nova_api present but exited — the CLI would fail, so gate first.
        state.services[1].status = ServiceStatus::NotRunning {
            podman_state: "exited".into(),
        };
        let ops = FakeInstanceOps::new().with_instances(sample_instances());
        let reply = handle_cloud_request(
            "list-instances",
            "",
            &state,
            &ops,
            &FakeCatalogSource::unconfigured(),
        );
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
            let reply = handle_cloud_request(
                verb,
                r#"{"instance":"i-9"}"#,
                &state,
                &ops,
                &FakeCatalogSource::unconfigured(),
            );
            assert!(reply.ok, "{verb}");
            assert_eq!(reply.instance.as_deref(), Some("i-9"), "{verb}");
            assert_eq!(ops.calls(), vec![want.to_string()], "{verb}");
        }
    }

    #[test]
    fn every_lifecycle_mutation_is_flagged_audited() {
        // #23 — every performed lifecycle verb is a control-plane mutation and
        // audits (start/stop change instance state as much as reboot/delete do);
        // a read is never audited.
        let state = enabled_state();
        for verb in [
            "instance-start",
            "instance-stop",
            "instance-reboot",
            "instance-delete",
        ] {
            let ops = FakeInstanceOps::new();
            let reply = handle_cloud_request(
                verb,
                r#"{"instance":"i-9"}"#,
                &state,
                &ops,
                &FakeCatalogSource::unconfigured(),
            );
            assert!(
                reply.ok && reply.audited,
                "{verb} is a mutation and must be audited (#23)"
            );
        }
        let read = handle_cloud_request(
            "list-instances",
            "",
            &state,
            &FakeInstanceOps::new(),
            &FakeCatalogSource::unconfigured(),
        );
        assert!(!read.audited, "a read is never audited");
    }

    #[test]
    fn verb_audits_agrees_with_the_bus_hash_chain_for_every_verb() {
        // #23 — the two audit layers must never disagree: every verb this flags an
        // audited mutation rides an auditable `action/cloud/<verb>` topic (its
        // request hash-chains into the KDC audit log), and every read carries the
        // exempt `get-`/`list-` stem. Proven verb-for-verb over the whole surface.
        for verb in CLOUD_VERBS {
            let topic = cloud_action_topic(verb);
            assert_eq!(
                verb_audits(verb),
                mde_bus::persist::is_auditable(&topic),
                "{verb}: verb_audits must match the Bus hash-chain guard"
            );
        }
        // Sanity: the mutations are the 4 lifecycle + 4 Heat verbs, nothing else.
        let audited = CLOUD_VERBS.into_iter().filter(|v| verb_audits(v)).count();
        assert_eq!(audited, 8, "exactly 8 mutating verbs audit");
    }

    #[test]
    fn a_destructive_verb_is_gated_and_never_performed_when_disabled() {
        // §7 — delete is performed ONLY when enabled + running; a disabled
        // doctrine gates it, and the seam is never touched (no eviction).
        let mut state = enabled_state();
        state.doctrine = DoctrineStatus::Disabled;
        let ops = FakeInstanceOps::new();
        let reply = handle_cloud_request(
            "instance-delete",
            r#"{"instance":"i-9"}"#,
            &state,
            &ops,
            &FakeCatalogSource::unconfigured(),
        );
        assert!(!reply.ok);
        assert!(reply.gated.is_some());
        assert!(!reply.audited);
        assert!(
            ops.calls().is_empty(),
            "a gated delete never reaches the seam"
        );
    }

    #[test]
    fn a_lifecycle_verb_without_an_instance_is_rejected() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        for body in ["", "{}", r#"{"instance":"  "}"#] {
            let reply = handle_cloud_request(
                "instance-stop",
                body,
                &state,
                &ops,
                &FakeCatalogSource::unconfigured(),
            );
            assert!(!reply.ok, "body {body:?}");
            assert!(reply.error.expect("error").contains("instance"));
            assert!(ops.calls().is_empty(), "no id ⇒ no seam call");
        }
    }

    #[test]
    fn a_malformed_lifecycle_body_is_a_typed_rejection_not_a_panic() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let reply = handle_cloud_request(
            "instance-start",
            "[1,2,3]",
            &state,
            &ops,
            &FakeCatalogSource::unconfigured(),
        );
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
        let reply = handle_cloud_request(
            "instance-start",
            r#"{"instance":"i-9"}"#,
            &state,
            &ops,
            &FakeCatalogSource::unconfigured(),
        );
        assert!(!reply.ok);
        assert!(reply.gated.is_none(), "a real failure is not a gate");
        assert!(reply.error.expect("error").contains("No server"));
        assert_eq!(
            ops.calls(),
            vec!["start:i-9".to_string()],
            "it did reach the seam"
        );
    }

    #[test]
    fn an_unknown_verb_is_rejected() {
        let state = enabled_state();
        let ops = FakeInstanceOps::new();
        let reply = handle_cloud_request(
            "instance-teleport",
            "{}",
            &state,
            &ops,
            &FakeCatalogSource::unconfigured(),
        );
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
            list[1]
                .networks
                .as_deref()
                .expect("networks")
                .contains("10.0.0.6"),
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
        let mut notifier = CloudNotifier::new("node-a");
        drain_cloud_verbs(
            &persist,
            &mut cursors,
            &state,
            &ops,
            &FakeCatalogSource::unconfigured(),
            &mut notifier,
        );

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
        // A routine success fires NO notify (#23 — only on failure/service-down).
        assert!(
            notify_bodies(&persist).is_empty(),
            "a successful lifecycle op must not notify"
        );

        // A second drain with the advanced cursor re-answers nothing (no
        // re-performed lifecycle op — the §7 idempotence of the cursor).
        drain_cloud_verbs(
            &persist,
            &mut cursors,
            &state,
            &ops,
            &FakeCatalogSource::unconfigured(),
            &mut notifier,
        );
        assert_eq!(ops.calls(), vec!["stop:i-42".to_string()], "no replay");
    }

    // ── IAC-5: the mesh notify feed (failure / service-down only) ──

    /// The bodies published to the cloud notify lane, oldest-first.
    fn notify_bodies(persist: &Persist) -> Vec<String> {
        persist
            .list_since(CLOUD_NOTIFY_TOPIC, None)
            .expect("list notify")
            .into_iter()
            .filter_map(|m| m.body)
            .collect()
    }

    /// Publish a request on `verb`'s topic + drain it once (a test helper — it
    /// simply forwards the drain's own seams, so the arg count mirrors it).
    #[allow(clippy::too_many_arguments)]
    fn publish_and_drain(
        persist: &Persist,
        cursors: &mut BTreeMap<String, Option<String>>,
        notifier: &mut CloudNotifier,
        state: &OpenStackState,
        ops: &dyn InstanceOps,
        client: &dyn CloudClient,
        verb: &str,
        body: &str,
    ) {
        persist
            .write(
                &cloud_action_topic(verb),
                Priority::Default,
                None,
                Some(body),
            )
            .expect("write request");
        drain_cloud_verbs(persist, cursors, state, ops, client, notifier);
    }

    #[test]
    fn a_failed_mutation_notifies_but_a_success_and_a_gate_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let state = enabled_state();
        let mut cursors = BTreeMap::new();
        let mut notifier = CloudNotifier::new("node-a");

        // A failing Heat mutation → one op-failure notify (warning).
        let failing = FakeCatalogSource::failing(super::super::client::ClientError::Transport(
            "heat 500".to_string(),
        ));
        publish_and_drain(
            &persist,
            &mut cursors,
            &mut notifier,
            &state,
            &FakeInstanceOps::new(),
            &failing,
            "heat-create",
            r#"{"stack_name":"web","template":"{}"}"#,
        );
        let after_fail = notify_bodies(&persist);
        assert_eq!(after_fail.len(), 1, "a failed mutation fires one notify");
        assert!(after_fail[0].contains("heat-create") && after_fail[0].contains("\"warning\""));
        assert!(
            after_fail[0].contains("\"source\":\"cloud\""),
            "the alert carries the cloud source badge"
        );

        // A successful Heat mutation → NO new notify.
        let ok = FakeCatalogSource::ok(ServiceCatalog::default(), vec![]);
        publish_and_drain(
            &persist,
            &mut cursors,
            &mut notifier,
            &state,
            &FakeInstanceOps::new(),
            &ok,
            "heat-create",
            r#"{"stack_name":"web2","template":"{}"}"#,
        );
        assert_eq!(
            notify_bodies(&persist).len(),
            1,
            "a routine success must not notify"
        );

        // A gated mutation (doctrine still enabled, but the seam gates via the
        // client's Unconfigured) → NO notify (retry-later, not a failure).
        let gated = FakeCatalogSource::unconfigured();
        publish_and_drain(
            &persist,
            &mut cursors,
            &mut notifier,
            &state,
            &FakeInstanceOps::new(),
            &gated,
            "heat-check",
            r#"{"stack_name":"web","stack_id":"s-1"}"#,
        );
        assert_eq!(
            notify_bodies(&persist).len(),
            1,
            "an honest gate must not notify"
        );

        // A failing lifecycle mutation → another op-failure notify.
        let bad_ops = FakeInstanceOps::new().failing("nova refused");
        publish_and_drain(
            &persist,
            &mut cursors,
            &mut notifier,
            &state,
            &bad_ops,
            &FakeCatalogSource::unconfigured(),
            "instance-delete",
            r#"{"instance":"i-9"}"#,
        );
        let bodies = notify_bodies(&persist);
        assert_eq!(bodies.len(), 2, "a failed lifecycle op fires a notify");
        assert!(bodies[1].contains("instance-delete"));
    }

    #[test]
    fn a_service_down_transition_notifies_but_first_sight_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).expect("persist");
        let state = enabled_state();
        let mut cursors = BTreeMap::new();
        let mut notifier = CloudNotifier::new("node-a");
        let ops = FakeInstanceOps::new();

        let catalog = sample_catalog_health().catalog;
        let up = vec![shape_health(
            "compute",
            EndpointInterface::Public,
            "http://nova.mesh:8774/v2.1",
            &ProbeOutcome::Reachable {
                http_status: 200,
                body: String::new(),
                elapsed_ms: 5,
            },
        )];
        let down = vec![shape_health(
            "compute",
            EndpointInterface::Public,
            "http://nova.mesh:8774/v2.1",
            &ProbeOutcome::Unreachable {
                elapsed_ms: 2000,
                reason: "connection refused".to_string(),
            },
        )];

        // First sight is UP → seeds the baseline silently (no notify).
        publish_and_drain(
            &persist,
            &mut cursors,
            &mut notifier,
            &state,
            &ops,
            &FakeCatalogSource::ok(catalog.clone(), up),
            "get-catalog",
            "",
        );
        assert!(
            notify_bodies(&persist).is_empty(),
            "first sight seeds silently"
        );

        // Now compute goes DOWN → one critical service-down notify.
        publish_and_drain(
            &persist,
            &mut cursors,
            &mut notifier,
            &state,
            &ops,
            &FakeCatalogSource::ok(catalog.clone(), down.clone()),
            "get-catalog",
            "",
        );
        let bodies = notify_bodies(&persist);
        assert_eq!(bodies.len(), 1, "an Up→Down transition fires one notify");
        assert!(bodies[0].contains("compute") && bodies[0].contains("\"critical\""));

        // Staying down does NOT re-notify (only the transition edge fires).
        publish_and_drain(
            &persist,
            &mut cursors,
            &mut notifier,
            &state,
            &ops,
            &FakeCatalogSource::ok(catalog, down),
            "get-catalog",
            "",
        );
        assert_eq!(
            notify_bodies(&persist).len(),
            1,
            "a still-down service does not re-notify"
        );
    }

    #[test]
    fn down_transitions_only_fires_on_a_prior_up() {
        use std::collections::BTreeMap as Map;
        let up = |ty: &str| (ty.to_string(), HealthState::Up);
        let down = |ty: &str| (ty.to_string(), HealthState::Down);
        let absent = |ty: &str| (ty.to_string(), HealthState::Absent);

        let prev: Map<String, HealthState> = [up("compute"), down("network"), absent("dns")].into();
        let now: Map<String, HealthState> = [
            down("compute"), // up → down: fires
            down("network"), // down → down: silent
            down("dns"),     // absent → down: silent
            down("image"),   // first sight down: silent
        ]
        .into();
        assert_eq!(down_transitions(&prev, &now), vec!["compute".to_string()]);
    }
}
