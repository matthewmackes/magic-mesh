//! Workloads U2 — the `cloud` worker's verb classifier + dispatch.
//!
//! [`CloudVerb`] classifies a drained `action/cloud/<verb>` token; [`dispatch`] is
//! the single match that routes a classified verb to its handler. The existing
//! verbs (list/status/configure) keep their behavior; legacy live provision is
//! explicitly refused, and retired instance lifecycle tokens are rejected before
//! classification, request parsing, authorization, or backend contact;
//! the U1a Workloads verbs (set-desired/plan/inventory/output/image-build/
//! android-provision/browser-provision) land here as typed handlers or honest
//! gates. Retired `container-deploy` remains classified only for an explicit
//! no-effect refusal. U4–U10
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
    CloudReply, CLOUD_ACTION_SCHEMA_VERSION, CLOUD_ARM_NODE_SCOPE, VERB_ANDROID_LIFECYCLE,
    VERB_ANDROID_PROVISION, VERB_APP_PROVISION, VERB_BROWSER_PROVISION, VERB_CONTAINER_DEPLOY,
    VERB_IMAGE_BUILD, VERB_INVENTORY, VERB_OUTPUT, VERB_PLAN, VERB_SET_DESIRED,
};

use super::runner::CloudRunOutcome;
use super::CloudWorker;

/// Cloud-owned instance lifecycle ended with the direct runner methods. Keep
/// these exact old wire tokens recognizable only long enough to return an
/// actionable no-effect compatibility refusal; they never become [`CloudVerb`]s.
const RETIRED_INSTANCE_LIFECYCLE_VERBS: &[&str] = &[
    "destroy",
    "instance-start",
    "instance-stop",
    "instance-reboot",
    "instance-delete",
    "instance-start-all",
    "instance-stop-all",
    "instance-reboot-all",
];

fn retired_instance_lifecycle_refusal(verb_name: &str) -> Option<CloudReply> {
    RETIRED_INSTANCE_LIFECYCLE_VERBS
        .contains(&verb_name)
        .then(|| CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some(format!(
                "cloud instance lifecycle verb `{verb_name}` is retired; use `action/workload/operation`"
            )),
            ..Default::default()
        })
}

/// The maximum action body accepted before JSON materialization. Direct callers
/// and Bus callers share the same RPC-sized boundary.
pub(crate) const MAX_CLOUD_ACTION_BODY_BYTES: usize = crate::ipc::MAX_RPC_BODY_BYTES;

// Disjoint per-verb handler modules (one unit each, `cloud/verbs/<unit>.rs`).
mod app_image;
mod container;
mod image;
// Disjoint per-verb handler modules (one unit each owns its file).
mod android; // U9 · android-provision
mod android_lifecycle; // WL-FUNC-020 S3 · typed Android VM/app lifecycle
mod app; // WL-FUNC-018 · app-provision
mod browser; // WL-ARCH-008 · browser-provision
mod inventory; // U10 · inventory + output

#[allow(unused_imports)] // Re-exported for sibling-module provider fixtures.
pub(crate) use android::{
    AndroidGuestProvider, AndroidGuestProviderRegistryError, CuttlefishProviderError,
};
pub(crate) use android::{
    AndroidGuestProviderRegistry, AndroidInventoryLedger, AndroidInventoryLedgerAdmission,
    AndroidInventoryLedgerError, CuttlefishOuterWorkloadObservation,
    WorkloadCuttlefishProviderClient,
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
    /// Retired live `provision` wire verb. Classified only for an explicit refusal.
    Provision,
    /// `configure` — `ansible-playbook` over the mesh inventory (MUTATION).
    Configure,
    /// `set-desired` — persist a node's desired-state doc (MUTATION; skeleton, U4).
    SetDesired,
    /// `image-build` — drive a bootc/osbuild image build (MUTATION; skeleton, U7).
    ImageBuild,
    /// Retired `container-deploy` wire verb, classified only for explicit refusal.
    ContainerDeploy,
    /// `android-provision` — the two-layer Cuttlefish path (MUTATION; skeleton, U10).
    AndroidProvision,
    /// `android-lifecycle` — start/stop/cancel/retry one admitted Android workload.
    AndroidLifecycle,
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
            v if v == VERB_INVENTORY => Self::Inventory,
            v if v == VERB_OUTPUT => Self::Output,
            v if v == VERB_PLAN => Self::Plan,
            v if v == VERB_SET_DESIRED => Self::SetDesired,
            v if v == VERB_IMAGE_BUILD => Self::ImageBuild,
            v if v == VERB_CONTAINER_DEPLOY => Self::ContainerDeploy,
            v if v == VERB_ANDROID_PROVISION => Self::AndroidProvision,
            v if v == VERB_ANDROID_LIFECYCLE => Self::AndroidLifecycle,
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
                | Self::SetDesired
                | Self::ImageBuild
                | Self::ContainerDeploy
                | Self::AndroidProvision
                | Self::AndroidLifecycle
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
    /// A verb-specific workload name (Android provision).
    #[serde(default)]
    pub name: Option<String>,
    /// Stable reverse-DNS Flatpak identity for `app-provision`.
    #[serde(default)]
    pub app_id: Option<String>,
    /// Catalog revision selected for the launch.
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
    /// Authenticated initiating peer whose shell will drive the App surface.
    #[serde(default)]
    pub client_peer: Option<String>,
    /// Resume an existing guest session when available.
    #[serde(default)]
    pub resume: bool,
    /// Immutable Browser VM guest-image digest (`sha256:<64-hex>`).
    #[serde(default)]
    pub image_digest: Option<String>,
    /// The armed-token capability authorizing a live mutation (mesh-identity-signed).
    #[serde(default)]
    pub armed_token: Option<String>,
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
    // This precedes classification and body parsing deliberately. The retired
    // tokens must not regain placement, schema, authorization, replay, or backend
    // semantics merely because an old publisher sends executable-looking JSON.
    if let Some(reply) = retired_instance_lifecycle_refusal(verb_name) {
        return reply;
    }
    let Some(verb) = CloudVerb::from_verb(verb_name) else {
        return CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some(format!("unknown cloud verb `{verb_name}`")),
            ..Default::default()
        };
    };
    // `raw` = the untouched wire body verb-specific handlers parse; `body` = the
    // shared gate fields. Retired handlers receive it only to return a refusal.
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

        // Image building remains a governed artifact mutation. Container deploy
        // is retained only as an explicit refusal; lifecycle/create belongs to
        // the typed Workload operation lane.
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
        CloudVerb::AndroidLifecycle => {
            let target = match android_lifecycle::authorization_target(raw) {
                Ok(target) => target,
                Err(error) => {
                    return CloudReply {
                        ok: false,
                        verb: verb_name.to_string(),
                        error: Some(error),
                        ..Default::default()
                    };
                }
            };
            if let Some(reply) = authorization_refusal(w, verb_name, &body, &target, raw) {
                return reply;
            }
            android_lifecycle::handle(w, verb_name, raw)
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

        // Live VM provisioning is owned exclusively by the typed Workload
        // operation lane. Keep the old verb classified so retained/out-of-tree
        // publishers receive an explicit refusal, but do not consume an armed
        // token, render mutable inputs, or contact a backend.
        CloudVerb::Provision => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some(
                "cloud provision is retired; use `action/workload/operation` for typed Workload provisioning"
                    .to_string(),
            ),
            ..Default::default()
        },
        CloudVerb::Configure => {
            if let Some(reply) =
                authorization_refusal(w, verb_name, &body, CLOUD_ARM_NODE_SCOPE, raw)
            {
                return reply;
            }
            let outcome = w.runner.configure();
            finish_authorized_mutation(verb_name, &outcome)
        }
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

/// The list/status read — serve the authoritative typed Workload roster or an
/// honest gate (never a fabricated empty roster or a direct backend probe).
fn handle_read_roster(w: &CloudWorker, verb_name: &str) -> CloudReply {
    match w.workload_instances() {
        Ok(instances) => CloudReply {
            ok: true,
            verb: verb_name.to_string(),
            instances: Some(instances),
            ..Default::default()
        },
        Err(e) => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            gated: Some(format!("Workload runtime authority not ready: {e}")),
            ..Default::default()
        },
    }
}

/// Turn an authorized backend mutation into its reply. Authorization has already
/// consumed the request's nonce before this function is reached.
fn finish_authorized_mutation(verb_name: &str, outcome: &CloudRunOutcome) -> CloudReply {
    let outcome = outcome.clone().require_live_apply(verb_name);
    if outcome.ok {
        CloudReply {
            ok: true,
            verb: verb_name.to_string(),
            ..Default::default()
        }
    } else {
        CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some(outcome.summary.clone()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_only_supported_cloud_verbs() {
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
        for verb in RETIRED_INSTANCE_LIFECYCLE_VERBS {
            assert_eq!(
                CloudVerb::from_verb(verb),
                None,
                "retired instance lifecycle token `{verb}` must have no executable classification"
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

        // Read/mutation classification covers only executable operations.
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
    }

    #[test]
    fn a_request_body_parses_the_placement_and_arming_fields() {
        let b = CloudActionBody::parse(
            r#"{"schema_version":1,"node":"eagle","name":"web","armed_token":"tok"}"#,
        );
        assert_eq!(b.schema_version, Some(CLOUD_ACTION_SCHEMA_VERSION));
        assert_eq!(b.node, "eagle");
        assert_eq!(b.name.as_deref(), Some("web"));
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

        let read_runner = Arc::new(FakeRunner::default());
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
}
