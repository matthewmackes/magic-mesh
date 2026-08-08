//! Workloads U2 — the `cloud` worker's verb classifier + dispatch.
//!
//! [`CloudVerb`] classifies a drained `action/cloud/<verb>` token; [`dispatch`] is
//! the single match that routes a classified verb to its handler. The existing
//! verbs (list/status/provision/configure/instance-*) keep their behavior; legacy
//! workspace-wide destroy is explicitly refused;
//! the U1a Workloads verbs (set-desired/plan/inventory/output/image-build/
//! container-deploy/android-provision/browser-provision) land here as
//! typed handlers or honest gates — recognized + routed, never faked (§7). U4–U10
//! each own one handler, so this dispatch is the worker's serialize point.
//!
//! The armed-token gate ([`super::gate`]) is applied here at APPLY time for the
//! implemented mutations; placement routing (which node dispatches at all) is the
//! drain's job in [`super`].

// U4 owns this verb handler (set-desired + plan); U6–U10 add their own disjoint
// `verbs/<unit>.rs` submodules here.
mod desired;

use serde::Deserialize;

use mackes_mesh_types::android_apps::{
    AndroidGuestInventoryRequest, AndroidGuestInventoryResponse,
};
use mackes_mesh_types::cloud::{
    CloudReply, CLOUD_ACTION_SCHEMA_VERSION, CLOUD_ARM_NODE_SCOPE, VERB_ANDROID_PROVISION,
    VERB_APP_PROVISION, VERB_BROWSER_PROVISION, VERB_CONTAINER_DEPLOY, VERB_IMAGE_BUILD,
    VERB_INVENTORY, VERB_OUTPUT, VERB_PLAN, VERB_SET_DESIRED,
};

use super::runner::{default_browser_vm_image_source, CloudRunOutcome};
use super::CloudWorker;

/// The maximum action body accepted before JSON materialization. Direct callers
/// and Bus callers share the same RPC-sized boundary.
pub(crate) const MAX_CLOUD_ACTION_BODY_BYTES: usize = crate::ipc::MAX_RPC_BODY_BYTES;

// Disjoint per-verb handler modules (one unit each, `cloud/verbs/<unit>.rs`).
mod app_image;
mod container;
mod image;
// Disjoint per-verb handler modules (one unit each owns its file).
mod android; // U9 · android-provision
mod app; // WL-FUNC-018 · app-provision
mod browser; // WL-ARCH-008 · browser-provision
mod inventory; // U10 · inventory + output

pub(crate) use android::{
    AndroidGuestProvider, AndroidGuestProviderRegistry, AndroidGuestProviderRegistryError,
    AndroidInventoryLedger, AndroidInventoryLedgerAdmission, AndroidInventoryLedgerError,
    LibvirtCuttlefishProviderClient,
};

impl CloudWorker {
    /// Admit one explicitly correlated guest inventory into the worker's
    /// bounded ledger. This is a crate-internal provider seam only: it accepts
    /// no commands, sockets, ADB data, or live-provider behavior.
    pub(crate) fn admit_android_inventory_response(
        &self,
        request: &AndroidGuestInventoryRequest,
        response: AndroidGuestInventoryResponse,
    ) -> Result<AndroidInventoryLedgerAdmission, AndroidInventoryLedgerError> {
        let mut ledger = self
            .android_inventory_ledger
            .lock()
            .map_err(|_| AndroidInventoryLedgerError::MutexPoisoned)?;
        let before = ledger.clone();
        let admission = ledger.admit_response(request, response)?;
        if matches!(
            admission,
            AndroidInventoryLedgerAdmission::Inserted | AndroidInventoryLedgerAdmission::Replaced
        ) {
            if let Some(path) = self.android_inventory_path.as_deref() {
                if let Err(error) = ledger.persist_to(path) {
                    // Do not expose an observation as retained if the durable
                    // journal could not be replaced. The previous snapshot is
                    // restored so a later publish cannot overstate durability.
                    *ledger = before;
                    return Err(error);
                }
            }
        }
        Ok(admission)
    }
}

/// A drained `action/cloud/<verb>` classified for dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloudVerb {
    /// `list` / `list-instances` — the instance roster (READ).
    List,
    /// `list-instances-local` — the instance roster for one explicit placement
    /// node (READ). This is the root KDC surface's non-fan-out roster query.
    LocalList,
    /// `status` — the roster + health summary (READ).
    Status,
    /// `inventory` — the resolved mesh Ansible inventory (READ; skeleton, U4).
    Inventory,
    /// `output` — the tofu outputs for a node's workloads (READ; skeleton, U5).
    Output,
    /// `plan` — the pending-change counts for a node's slice (READ; skeleton, U5).
    Plan,
    /// `provision` — `tofu plan/apply` in `infra/tofu/cloud` (MUTATION).
    Provision,
    /// `configure` — `ansible-playbook` over the mesh inventory (MUTATION).
    Configure,
    /// Retired workspace-wide `destroy` wire verb. Kept classified only so old
    /// clients receive an explicit refusal instead of an unknown-verb ambiguity.
    Destroy,
    /// `set-desired` — persist a node's desired-state doc (MUTATION; skeleton, U4).
    SetDesired,
    /// `image-build` — drive a bootc/osbuild image build (MUTATION; skeleton, U7).
    ImageBuild,
    /// `container-deploy` — render + hand off a Quadlet unit (MUTATION; skeleton, U8).
    ContainerDeploy,
    /// `android-provision` — the two-layer Cuttlefish path (MUTATION; skeleton, U10).
    AndroidProvision,
    /// `browser-provision` — declare the dedicated Desktop VM browser workload.
    BrowserProvision,
    /// `app-provision` — declare one admitted guest-owned Flatpak App VM.
    AppProvision,
}

impl CloudVerb {
    /// Classify a verb token, or `None` for an unrecognized verb (never guessed).
    #[must_use]
    pub fn from_verb(verb: &str) -> Option<Self> {
        Some(match verb {
            "list" | "list-instances" => Self::List,
            "list-instances-local" => Self::LocalList,
            "status" => Self::Status,
            "provision" => Self::Provision,
            "configure" => Self::Configure,
            "destroy" => Self::Destroy,
            v if v == VERB_INVENTORY => Self::Inventory,
            v if v == VERB_OUTPUT => Self::Output,
            v if v == VERB_PLAN => Self::Plan,
            v if v == VERB_SET_DESIRED => Self::SetDesired,
            v if v == VERB_IMAGE_BUILD => Self::ImageBuild,
            v if v == VERB_CONTAINER_DEPLOY => Self::ContainerDeploy,
            v if v == VERB_ANDROID_PROVISION => Self::AndroidProvision,
            v if v == VERB_BROWSER_PROVISION => Self::BrowserProvision,
            v if v == VERB_APP_PROVISION => Self::AppProvision,
            _ => return None,
        })
    }

    /// Whether this verb mutates backend state (so it rides the armed-token gate
    /// AND the placement gate — a mutation is performed only on its placement node).
    /// Reads (`list`/`status`/`inventory`/`output`/`plan`) are served locally on
    /// every node.
    #[must_use]
    pub const fn is_mutation(self) -> bool {
        matches!(
            self,
            Self::Provision
                | Self::Configure
                | Self::Destroy
                | Self::SetDesired
                | Self::ImageBuild
                | Self::ContainerDeploy
                | Self::AndroidProvision
                | Self::BrowserProvision
                | Self::AppProvision
        )
    }

    /// Whether this action must be routed to exactly one explicit placement
    /// node. Inventory/output/plan read node-local state, so they are scoped
    /// just like mutations; list/status remain intentionally local reads.
    #[must_use]
    pub const fn requires_placement(self) -> bool {
        self.is_mutation()
            || matches!(
                self,
                Self::LocalList | Self::Inventory | Self::Output | Self::Plan
            )
    }

    /// Whether performing this verb is destructive (`destroy` / a destructive
    /// lifecycle op) — the ops audited on the events plane when performed (§7).
    #[must_use]
    pub const fn is_destructive(self) -> bool {
        match self {
            Self::Destroy => true,
            _ => false,
        }
    }
}

/// The parsed `action/cloud/*` request body — the fields the worker reads off the
/// wire JSON. Every field is optional so read-only requests can retain their
/// legacy shape; [`Self::schema_error_for`] applies the v1 envelope requirement
/// to placement-scoped mutations before any handler or backend is reached.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CloudActionBody {
    /// Explicit request-envelope version. Mutations must carry exactly v1;
    /// read-only requests may omit it for compatibility.
    #[serde(default)]
    pub schema_version: Option<u16>,
    /// The placement node this request targets (the placement gate's key).
    /// Mutations require a non-empty explicit value.
    #[serde(default)]
    pub node: String,
    /// A lifecycle op's target instance/domain name.
    #[serde(default)]
    pub instance: Option<String>,
    /// A verb-specific workload name (Android provision and console attach).
    #[serde(default)]
    pub name: Option<String>,
    /// Stable reverse-DNS Flatpak identity for `app-provision`.
    #[serde(default)]
    pub app_id: Option<String>,
    /// Signed catalog revision selected for the launch.
    #[serde(default)]
    pub catalog_revision: Option<String>,
    /// Approved named guest profile.
    #[serde(default)]
    pub guest_profile: Option<String>,
    /// Capabilities requested by the app declaration.
    #[serde(default)]
    pub requested_capabilities: Vec<String>,
    /// Stable session identity used to converge repeated launches.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Resume an existing guest session when available.
    #[serde(default)]
    pub resume: bool,
    /// Immutable Browser VM guest-image digest (`sha256:<64-hex>`).
    #[serde(default)]
    pub image_digest: Option<String>,
    /// The armed-token capability authorizing a live mutation (mesh-identity-signed).
    #[serde(default)]
    pub armed_token: Option<String>,
    /// The typed-arming confirmation a destructive lifecycle request carries.
    #[serde(default)]
    pub typed_name: Option<String>,
    /// Set only when the wire body could not be parsed. This is deliberately
    /// outside the wire schema so a malformed body cannot be mistaken for the
    /// valid legacy `{}` read envelope.
    #[serde(skip)]
    parse_error: Option<String>,
}

impl CloudActionBody {
    /// Parse a request body while retaining malformed-input state for the shared
    /// dispatch gate. Valid legacy `{}` read requests still parse as an empty
    /// body; malformed JSON never gets that compatibility treatment.
    #[must_use]
    pub fn parse(body: &str) -> Self {
        if body.len() > MAX_CLOUD_ACTION_BODY_BYTES {
            return Self {
                parse_error: Some(format!(
                    "cloud action body exceeds {MAX_CLOUD_ACTION_BODY_BYTES}-byte limit"
                )),
                ..Default::default()
            };
        }
        match serde_json::from_str(body.trim()) {
            Ok(parsed) => parsed,
            Err(_) => Self {
                parse_error: Some("cloud action body must be valid JSON".to_string()),
                ..Default::default()
            },
        }
    }

    /// Refuse an unsupported envelope, or a missing envelope on a mutation.
    ///
    /// Placement-scoped reads (including the dry-run `plan`) intentionally keep
    /// accepting an omitted version. An explicit future version is refused for
    /// every verb because the worker cannot safely interpret its fields.
    #[must_use]
    pub(crate) fn schema_error_for(&self, verb: CloudVerb) -> Option<String> {
        if let Some(error) = &self.parse_error {
            return Some(error.clone());
        }
        match self.schema_version {
            Some(version) if version != CLOUD_ACTION_SCHEMA_VERSION => Some(format!(
                "unsupported cloud request schema version {version} (expected {CLOUD_ACTION_SCHEMA_VERSION})"
            )),
            None if verb.is_mutation() => Some(format!(
                "cloud mutation requires schema_version {CLOUD_ACTION_SCHEMA_VERSION}"
            )),
            _ => None,
        }
    }
}

/// Route a classified `action/cloud/<verb>` request end to end → a typed
/// [`CloudReply`]. Reads serve the roster (or an honest skeleton `not-yet`);
/// implemented mutations run the armed-token gate; skeleton mutations return an
/// honest `not-yet-wired`. Never panics.
pub(crate) fn dispatch(w: &CloudWorker, verb_name: &str, body_str: &str) -> CloudReply {
    let Some(verb) = CloudVerb::from_verb(verb_name) else {
        return CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some(format!("unknown cloud verb `{verb_name}`")),
            ..Default::default()
        };
    };
    // `raw` = the untouched wire body the image-build/container-deploy handlers
    // parse their verb-specific fields from; `body` = the shared gate fields.
    let raw = body_str;
    let body = CloudActionBody::parse(body_str);
    if let Some(error) = body.schema_error_for(verb) {
        return CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some(error),
            ..Default::default()
        };
    }
    if verb.requires_placement() && body.node.trim().is_empty() {
        return CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some("cloud action requires an explicit placement `node`".to_string()),
            ..Default::default()
        };
    }

    match verb {
        // ── implemented READS — served locally on every node ──
        CloudVerb::List | CloudVerb::LocalList | CloudVerb::Status => {
            handle_read_roster(w, verb_name)
        }

        // ── implemented READS — served locally on every node (U10) ──
        CloudVerb::Inventory => inventory::handle_inventory(w, verb_name),
        CloudVerb::Output => inventory::handle_output(w, verb_name),
        // U4 — `set-desired` persists the node's desired doc; `plan` renders its
        // slice + shells `tofu plan -json` → PlanCounts (honest gate on failure).
        CloudVerb::Plan => desired::handle_plan(w, verb_name, body_str),
        CloudVerb::SetDesired => {
            let target = match desired::authorization_target(raw) {
                Ok(target) => target,
                Err(error) => {
                    return CloudReply {
                        ok: false,
                        verb: verb_name.to_string(),
                        error: Some(error),
                        ..Default::default()
                    }
                }
            };
            if let Some(reply) = authorization_refusal(w, verb_name, &body, &target, raw) {
                return reply;
            }
            desired::handle_set_desired(w, verb_name, body_str)
        }

        // ── wired MUTATIONS — image-build (U6) + container-deploy (U7) ──
        CloudVerb::ImageBuild => image::handle(w, verb_name, raw),
        CloudVerb::ContainerDeploy => container::handle(w, verb_name, raw),
        // Container and VM day-2 lifecycle is exclusively owned by the typed
        // Workload operation lane. These legacy cloud verbs are deliberately
        // unclassified, so old publishers receive an unknown-verb refusal
        // before auth, replay, or any backend can be reached.

        // ── wired MUTATIONS — android-provision (U9), browser-provision
        // (WL-ARCH-008). Presentation attachment is owned exclusively by the
        // typed Workload Open/StartAndAttach lane; the retired cloud
        // `console-attach` verb is intentionally unclassified.
        CloudVerb::AndroidProvision => {
            let target = android::authorization_target(&body);
            if let Some(reply) = authorization_refusal(w, verb_name, &body, &target, raw) {
                return reply;
            }
            android::handle(w, verb_name, &body)
        }
        CloudVerb::BrowserProvision => {
            let target = match browser::authorization_target(&body) {
                Ok(target) => target,
                Err(error) => {
                    return CloudReply {
                        ok: false,
                        verb: verb_name.to_string(),
                        error: Some(error),
                        ..Default::default()
                    }
                }
            };
            if let Some(reply) = authorization_refusal(w, verb_name, &body, target, raw) {
                return reply;
            }
            browser::handle(w, verb_name, &body)
        }
        CloudVerb::AppProvision => {
            let target = match app::authorization_target(&body) {
                Ok(target) => target,
                Err(error) => {
                    return CloudReply {
                        ok: false,
                        verb: verb_name.to_string(),
                        error: Some(error),
                        ..Default::default()
                    }
                }
            };
            if let Some(reply) = authorization_refusal(w, verb_name, &body, &target, raw) {
                return reply;
            }
            app::handle(w, verb_name, &body)
        }

        // ── implemented MUTATIONS — the armed-token gate ──
        CloudVerb::Provision => {
            if let Some(reply) =
                authorization_refusal(w, verb_name, &body, CLOUD_ARM_NODE_SCOPE, raw)
            {
                return reply;
            }
            let tfvars = match super::reconcile::rendered_tfvars_for_node(
                &w.state_root,
                body.node.trim(),
                &super::runner::default_libvirt_uri(),
                &default_browser_vm_image_source(),
            ) {
                Ok(tfvars) => tfvars,
                Err(error) => {
                    return CloudReply {
                        ok: false,
                        verb: verb_name.to_string(),
                        error: Some(format!(
                            "provision desired state could not be rendered: {error}"
                        )),
                        ..Default::default()
                    }
                }
            };
            let outcome = w.runner.provision(&tfvars);
            finish_authorized_mutation(w, verb, verb_name, &outcome, None)
        }
        CloudVerb::Configure => {
            if let Some(reply) =
                authorization_refusal(w, verb_name, &body, CLOUD_ARM_NODE_SCOPE, raw)
            {
                return reply;
            }
            let outcome = w.runner.configure();
            finish_authorized_mutation(w, verb, verb_name, &outcome, None)
        }
        CloudVerb::Destroy => handle_destroy(w, verb_name, &body),
    }
}

fn authorization_refusal(
    w: &CloudWorker,
    verb_name: &str,
    body: &CloudActionBody,
    target: &str,
    raw: &str,
) -> Option<CloudReply> {
    let verdict = w.consume_armed_token(
        body.armed_token.as_deref(),
        verb_name,
        body.node.trim(),
        target,
        raw,
    );
    (!verdict.is_valid()).then(|| CloudReply {
        ok: false,
        verb: verb_name.to_string(),
        gated: Some(format!(
            "cloud action is not authorized ({}) — nothing changed or disclosed",
            verdict.reason()
        )),
        ..Default::default()
    })
}

/// The list/status read — serve the live roster or an honest gate (never a
/// fabricated empty roster).
fn handle_read_roster(w: &CloudWorker, verb_name: &str) -> CloudReply {
    match w.runner.list_instances() {
        Ok(instances) => CloudReply {
            ok: true,
            verb: verb_name.to_string(),
            instances: Some(instances),
            ..Default::default()
        },
        Err(e) => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            gated: Some(format!("cloud backend not ready: {e}")),
            ..Default::default()
        },
    }
}

/// Workspace-wide destruction is not a valid Workloads operation. Old clients
/// get an explicit fail-closed reply and must select a row, which routes through
/// target-scoped `instance-delete`.
fn handle_destroy(_w: &CloudWorker, verb_name: &str, _body: &CloudActionBody) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb_name.to_string(),
        error: Some(
            "workspace-wide destroy is retired; use target-scoped `instance-delete`".to_string(),
        ),
        ..Default::default()
    }
}

/// Turn an authorized backend mutation into its reply. Authorization has already
/// consumed the request's nonce before this function is reached; destructive
/// operations are audited so `audited: true` remains truthful.
fn finish_authorized_mutation(
    w: &CloudWorker,
    verb: CloudVerb,
    verb_name: &str,
    outcome: &CloudRunOutcome,
    instance: Option<&str>,
) -> CloudReply {
    let outcome = outcome.clone().require_live_apply(verb_name);
    let audited = verb.is_destructive();
    if audited {
        w.audit(verb_name, instance, &outcome);
    }
    if outcome.ok {
        CloudReply {
            ok: true,
            verb: verb_name.to_string(),
            audited,
            ..Default::default()
        }
    } else {
        CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some(outcome.summary.clone()),
            audited,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_supported_verbs_and_retires_legacy_lifecycle_topics() {
        assert_eq!(CloudVerb::from_verb("list"), Some(CloudVerb::List));
        assert_eq!(
            CloudVerb::from_verb("list-instances"),
            Some(CloudVerb::List)
        );
        assert_eq!(CloudVerb::from_verb("status"), Some(CloudVerb::Status));
        assert_eq!(
            CloudVerb::from_verb("list-instances-local"),
            Some(CloudVerb::LocalList)
        );
        assert_eq!(
            CloudVerb::from_verb("provision"),
            Some(CloudVerb::Provision)
        );
        for verb in [
            "instance-start",
            "instance-stop",
            "instance-reboot",
            "instance-delete",
            "instance-start-all",
            "instance-stop-all",
            "instance-reboot-all",
            "container-restart",
            "container-logs",
            "container-destroy",
        ] {
            assert_eq!(
                CloudVerb::from_verb(verb),
                None,
                "legacy lifecycle topic `{verb}` must be unclassified"
            );
        }
        // U1a Workloads verbs classify (they were unknown before U2).
        assert_eq!(
            CloudVerb::from_verb("set-desired"),
            Some(CloudVerb::SetDesired)
        );
        assert_eq!(CloudVerb::from_verb("plan"), Some(CloudVerb::Plan));
        assert_eq!(
            CloudVerb::from_verb("inventory"),
            Some(CloudVerb::Inventory)
        );
        assert_eq!(
            CloudVerb::from_verb("android-provision"),
            Some(CloudVerb::AndroidProvision)
        );
        assert_eq!(
            CloudVerb::from_verb("browser-provision"),
            Some(CloudVerb::BrowserProvision)
        );
        assert_eq!(
            CloudVerb::from_verb("app-provision"),
            Some(CloudVerb::AppProvision)
        );
        assert_eq!(CloudVerb::from_verb("frobnicate"), None);

        // read/mutation/destructive classification.
        assert!(!CloudVerb::List.is_mutation());
        assert!(!CloudVerb::Inventory.is_mutation());
        assert!(!CloudVerb::Plan.is_mutation());
        assert!(CloudVerb::Inventory.requires_placement());
        assert!(CloudVerb::LocalList.requires_placement());
        assert!(CloudVerb::Output.requires_placement());
        assert!(CloudVerb::Plan.requires_placement());
        assert!(!CloudVerb::List.requires_placement());
        assert!(CloudVerb::Provision.is_mutation());
        assert!(CloudVerb::SetDesired.is_mutation());
        assert!(CloudVerb::AndroidProvision.is_mutation());
        assert!(CloudVerb::BrowserProvision.is_mutation());
        assert!(CloudVerb::AppProvision.is_mutation());
        assert!(CloudVerb::Destroy.is_destructive());
        assert!(!CloudVerb::Provision.is_destructive());
    }

    #[test]
    fn a_request_body_parses_the_placement_and_arming_fields() {
        let b = CloudActionBody::parse(
            r#"{"schema_version":1,"node":"eagle","instance":"web","armed_token":"tok","typed_name":"web"}"#,
        );
        assert_eq!(b.schema_version, Some(CLOUD_ACTION_SCHEMA_VERSION));
        assert_eq!(b.node, "eagle");
        assert_eq!(b.instance.as_deref(), Some("web"));
        assert_eq!(b.armed_token.as_deref(), Some("tok"));
        // A malformed body is retained as an explicit parse failure rather than
        // being treated as the valid legacy empty read envelope.
        let empty = CloudActionBody::parse("not json");
        assert!(empty.node.is_empty() && empty.armed_token.is_none());
        assert!(empty
            .schema_error_for(CloudVerb::List)
            .is_some_and(|error| error.contains("valid JSON")));
    }

    #[test]
    fn oversized_action_body_is_rejected_before_json_materialization() {
        let body = "{".repeat(MAX_CLOUD_ACTION_BODY_BYTES + 1);
        let parsed = CloudActionBody::parse(&body);
        assert_eq!(
            parsed.schema_error_for(CloudVerb::ContainerDeploy),
            Some(format!(
                "cloud action body exceeds {MAX_CLOUD_ACTION_BODY_BYTES}-byte limit"
            ))
        );
    }

    #[test]
    fn mutations_require_explicit_v1_but_reads_and_plan_keep_legacy_shape() {
        let missing = CloudActionBody::parse(r#"{"node":"eagle"}"#);
        let error = missing
            .schema_error_for(CloudVerb::SetDesired)
            .expect("missing mutation schema must fail closed");
        assert!(error.contains("requires schema_version 1"), "{error}");

        let future = CloudActionBody::parse(r#"{"schema_version":2,"node":"eagle"}"#);
        let error = future
            .schema_error_for(CloudVerb::SetDesired)
            .expect("future mutation schema must fail closed");
        assert!(
            error.contains("unsupported cloud request schema version 2"),
            "{error}"
        );

        let v1 = CloudActionBody::parse(r#"{"schema_version":1,"node":"eagle"}"#);
        assert!(v1.schema_error_for(CloudVerb::SetDesired).is_none());
        assert!(missing.schema_error_for(CloudVerb::Inventory).is_none());
        assert!(missing.schema_error_for(CloudVerb::Plan).is_none());
        assert!(missing.schema_error_for(CloudVerb::List).is_none());
    }

    #[test]
    fn malformed_json_is_refused_before_read_or_mutation_handlers() {
        use std::path::PathBuf;
        use std::sync::Arc;

        use super::super::runner::fake::FakeRunner;
        use super::super::CloudWorker;

        let read_runner = Arc::new(FakeRunner {
            roster_err: Some("roster handler must not run".to_string()),
            ..Default::default()
        });
        let read_worker = CloudWorker::new(
            "me".into(),
            "peer:me".into(),
            PathBuf::from("/tmp/mackesd-cloud-verbs-test"),
        )
        .with_runner(read_runner);
        let read_reply = read_worker.handle("list", "{\"node\":");
        assert!(!read_reply.ok);
        assert!(read_reply
            .error
            .as_deref()
            .is_some_and(|error| error.contains("valid JSON")));
        assert!(read_reply.gated.is_none());

        let mutation_runner = Arc::new(FakeRunner::default());
        let mutation_worker = CloudWorker::new(
            "me".into(),
            "peer:me".into(),
            PathBuf::from("/tmp/mackesd-cloud-verbs-test"),
        )
        .with_runner(mutation_runner.clone());
        let mutation_reply = mutation_worker.handle("set-desired", "[");
        assert!(!mutation_reply.ok);
        assert!(mutation_reply
            .error
            .as_deref()
            .is_some_and(|error| error.contains("valid JSON")));
        assert!(mutation_runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn retired_lifecycle_topics_are_refused_before_auth_or_backend() {
        use std::path::PathBuf;
        use std::sync::Arc;

        use super::super::runner::fake::FakeRunner;
        use super::super::CloudWorker;

        let runner = Arc::new(FakeRunner::default());
        let worker = CloudWorker::new(
            "me".into(),
            "peer:me".into(),
            PathBuf::from("/tmp/mackesd-cloud-verbs-lifecycle-test"),
        )
        .with_runner(runner.clone());

        for verb in [
            "instance-start",
            "instance-stop",
            "instance-reboot",
            "instance-delete",
            "container-restart",
            "container-logs",
            "container-destroy",
        ] {
            let body = r#"{"schema_version":1,"node":"me","instance":"web"}"#;
            let reply = worker.handle(verb, &body);
            assert!(!reply.ok, "{verb} must be rejected");
            assert!(reply
                .error
                .as_deref()
                .is_some_and(|error| error.contains("unknown cloud verb")));
        }
        assert!(runner.calls.lock().unwrap().is_empty());
    }
}
