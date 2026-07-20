//! Workloads U2 — the `cloud` worker's verb classifier + dispatch.
//!
//! [`CloudVerb`] classifies a drained `action/cloud/<verb>` token; [`dispatch`] is
//! the single match that routes a classified verb to its handler. The existing
//! verbs (list/status/provision/configure/destroy/instance-*) keep their behavior;
//! the U1a Workloads verbs (set-desired/plan/inventory/output/image-build/
//! container-deploy/console-attach/android-provision) land here as honest
//! `not-yet-wired` skeletons — recognized + routed, never faked (§7). U4–U10 each
//! own one skeleton handler, so this dispatch is the worker's serialize point.
//!
//! The armed-token gate ([`super::gate`]) is applied here at APPLY time for the
//! implemented mutations; placement routing (which node dispatches at all) is the
//! drain's job in [`super`].

// U4 owns this verb handler (set-desired + plan); U6–U10 add their own disjoint
// `verbs/<unit>.rs` submodules here.
mod desired;

use serde::Deserialize;

use mackes_mesh_types::cloud::{
    CloudReply, LifecycleAction, VERB_ANDROID_PROVISION, VERB_CONSOLE_ATTACH,
    VERB_CONTAINER_DEPLOY, VERB_IMAGE_BUILD, VERB_INVENTORY, VERB_OUTPUT, VERB_PLAN,
    VERB_SET_DESIRED,
};

use super::gate::{self, CloudDecision, TokenVerdict};
use super::runner::CloudRunOutcome;
use super::CloudWorker;

/// A drained `action/cloud/<verb>` classified for dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloudVerb {
    /// `list` / `list-instances` — the instance roster (READ).
    List,
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
    /// `destroy` — `tofu plan -destroy` / `destroy` (MUTATION; typed-arming).
    Destroy,
    /// `instance-{start,stop,reboot,delete}` — a `virsh` domain op (MUTATION).
    Lifecycle(LifecycleAction),
    /// `set-desired` — persist a node's desired-state doc (MUTATION; skeleton, U4).
    SetDesired,
    /// `image-build` — drive a bootc/osbuild image build (MUTATION; skeleton, U7).
    ImageBuild,
    /// `container-deploy` — render + hand off a Quadlet unit (MUTATION; skeleton, U8).
    ContainerDeploy,
    /// `console-attach` — a SPICE/VNC console handle (MUTATION-placed; skeleton, U9).
    ConsoleAttach,
    /// `android-provision` — the two-layer Cuttlefish path (MUTATION; skeleton, U10).
    AndroidProvision,
}

impl CloudVerb {
    /// Classify a verb token, or `None` for an unrecognized verb (never guessed).
    #[must_use]
    pub fn from_verb(verb: &str) -> Option<Self> {
        if let Some(action) = LifecycleAction::from_verb(verb) {
            return Some(Self::Lifecycle(action));
        }
        Some(match verb {
            "list" | "list-instances" => Self::List,
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
            v if v == VERB_CONSOLE_ATTACH => Self::ConsoleAttach,
            v if v == VERB_ANDROID_PROVISION => Self::AndroidProvision,
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
                | Self::Lifecycle(_)
                | Self::SetDesired
                | Self::ImageBuild
                | Self::ContainerDeploy
                | Self::ConsoleAttach
                | Self::AndroidProvision
        )
    }

    /// Whether performing this verb is destructive (`destroy` / a destructive
    /// lifecycle op) — the ops audited on the events plane when performed (§7).
    #[must_use]
    pub const fn is_destructive(self) -> bool {
        match self {
            Self::Destroy => true,
            Self::Lifecycle(a) => a.is_destructive(),
            _ => false,
        }
    }
}

/// The parsed `action/cloud/*` request body — the fields the worker reads off the
/// wire JSON. Every field is optional so a legacy `{}` request still parses; the
/// per-verb handlers enforce what each actually requires.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CloudActionBody {
    /// The placement node this request targets (the placement gate's key). Empty ⇒
    /// node-agnostic (the legacy KDC lifecycle path).
    #[serde(default)]
    pub node: String,
    /// A lifecycle op's target instance/domain name.
    #[serde(default)]
    pub instance: Option<String>,
    /// A destroy's target workload name (falls back to `node` when unset).
    #[serde(default)]
    pub name: Option<String>,
    /// The armed-token capability authorizing a live mutation (mesh-identity-signed).
    #[serde(default)]
    pub armed_token: Option<String>,
    /// The typed-arming confirmation a `destroy` must carry (== the destroy target).
    #[serde(default)]
    pub typed_name: Option<String>,
}

impl CloudActionBody {
    /// Parse a request body, degrading a malformed body to an all-empty request
    /// (the per-verb handlers then honestly reject what they require).
    #[must_use]
    pub fn parse(body: &str) -> Self {
        serde_json::from_str(body.trim()).unwrap_or_default()
    }

    /// The destroy target: the explicit `name`, else the placement `node`.
    fn destroy_target(&self) -> &str {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.node.trim())
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
    let body = CloudActionBody::parse(body_str);

    match verb {
        // ── implemented READS — served locally on every node ──
        CloudVerb::List | CloudVerb::Status => handle_read_roster(w, verb_name),

        // ── skeleton READS (U10 fills the bodies) ──
        CloudVerb::Inventory => not_yet(verb_name, "U4"),
        CloudVerb::Output => not_yet(verb_name, "U5"),
        // U4 — `set-desired` persists the node's desired doc; `plan` renders its
        // slice + shells `tofu plan -json` → PlanCounts (honest gate on failure).
        CloudVerb::Plan => desired::handle_plan(w, verb_name, body_str),
        CloudVerb::SetDesired => desired::handle_set_desired(w, verb_name, body_str),

        // ── skeleton MUTATIONS (U4–U10 fill the bodies) ──
        CloudVerb::ImageBuild => not_yet(verb_name, "U7"),
        CloudVerb::ContainerDeploy => not_yet(verb_name, "U8"),
        CloudVerb::ConsoleAttach => not_yet(verb_name, "U9"),
        CloudVerb::AndroidProvision => not_yet(verb_name, "U10"),

        // ── implemented MUTATIONS — the armed-token gate ──
        CloudVerb::Provision => {
            let (verdict, decision) = gate_decision(w, verb, verb_name, &body);
            let outcome = w.runner.provision(matches!(decision, CloudDecision::Apply));
            finish_mutation(w, verb, verb_name, decision, verdict, &outcome, None)
        }
        CloudVerb::Configure => {
            let (verdict, decision) = gate_decision(w, verb, verb_name, &body);
            let outcome = w.runner.configure(matches!(decision, CloudDecision::Apply));
            finish_mutation(w, verb, verb_name, decision, verdict, &outcome, None)
        }
        CloudVerb::Destroy => handle_destroy(w, verb_name, &body),
        CloudVerb::Lifecycle(action) => handle_lifecycle(w, verb, verb_name, action, &body),
    }
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

/// An honest `not-yet-wired` skeleton reply — recognized + routed, never faked (§7).
fn not_yet(verb_name: &str, unit: &str) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb_name.to_string(),
        gated: Some(format!(
            "cloud verb `{verb_name}` is recognized but not yet wired ({unit})"
        )),
        ..Default::default()
    }
}

/// Compute the armed-token verdict + the apply/stage decision for a mutation.
fn gate_decision(
    w: &CloudWorker,
    verb: CloudVerb,
    verb_name: &str,
    body: &CloudActionBody,
) -> (TokenVerdict, CloudDecision) {
    let verdict = gate::verify_token(
        body.armed_token.as_deref(),
        verb_name,
        body.node.trim(),
        super::now_ms(),
        w.signer.as_ref(),
    );
    (verdict, gate::decide(verb, verdict.is_valid()))
}

/// The lifecycle mutation — resolves the target instance, then runs the gate.
fn handle_lifecycle(
    w: &CloudWorker,
    verb: CloudVerb,
    verb_name: &str,
    action: LifecycleAction,
    body: &CloudActionBody,
) -> CloudReply {
    let Some(instance) = body
        .instance
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some(format!(
                "`{verb_name}` requires an `instance` field in the request body"
            )),
            ..Default::default()
        };
    };
    let (verdict, decision) = gate_decision(w, verb, verb_name, body);
    let outcome = w
        .runner
        .lifecycle(action, instance, matches!(decision, CloudDecision::Apply));
    finish_mutation(
        w,
        verb,
        verb_name,
        decision,
        verdict,
        &outcome,
        Some(instance),
    )
}

/// The destroy mutation — the armed-token gate PLUS the typed-arming confirmation
/// (`typed_name` must equal the destroy target when applying live).
fn handle_destroy(w: &CloudWorker, verb_name: &str, body: &CloudActionBody) -> CloudReply {
    let (verdict, decision) = gate_decision(w, CloudVerb::Destroy, verb_name, body);
    if matches!(decision, CloudDecision::Apply) {
        let target = body.destroy_target();
        let confirmed = body
            .typed_name
            .as_deref()
            .map(str::trim)
            .is_some_and(|t| !t.is_empty() && !target.is_empty() && t == target);
        if !confirmed {
            return CloudReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some(format!(
                    "destroy blocked: `typed_name` must equal the destroy target `{target}` \
                     (typed-arming confirmation)"
                )),
                ..Default::default()
            };
        }
    }
    let outcome = w.runner.destroy(matches!(decision, CloudDecision::Apply));
    finish_mutation(
        w,
        CloudVerb::Destroy,
        verb_name,
        decision,
        verdict,
        &outcome,
        None,
    )
}

/// Turn a decided + run mutation into its reply: a staged run is honestly gated
/// (nothing applied); an applied destructive op is audited so `audited: true` is
/// truthful.
fn finish_mutation(
    w: &CloudWorker,
    verb: CloudVerb,
    verb_name: &str,
    decision: CloudDecision,
    verdict: TokenVerdict,
    outcome: &CloudRunOutcome,
    instance: Option<&str>,
) -> CloudReply {
    match decision {
        CloudDecision::Staged => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            gated: Some(format!(
                "live apply is gated ({}) — {} — nothing applied",
                verdict.reason(),
                outcome.summary
            )),
            ..Default::default()
        },
        CloudDecision::Apply => {
            let audited = verb.is_destructive();
            if audited {
                w.audit(verb_name, instance, outcome);
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
        CloudDecision::Read => unreachable!("reads are handled before finish_mutation"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_reads_mutations_lifecycle_and_the_workloads_skeleton_verbs() {
        assert_eq!(CloudVerb::from_verb("list"), Some(CloudVerb::List));
        assert_eq!(
            CloudVerb::from_verb("list-instances"),
            Some(CloudVerb::List)
        );
        assert_eq!(CloudVerb::from_verb("status"), Some(CloudVerb::Status));
        assert_eq!(
            CloudVerb::from_verb("provision"),
            Some(CloudVerb::Provision)
        );
        assert_eq!(
            CloudVerb::from_verb("instance-reboot"),
            Some(CloudVerb::Lifecycle(LifecycleAction::Reboot))
        );
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
        assert_eq!(CloudVerb::from_verb("frobnicate"), None);

        // read/mutation/destructive classification.
        assert!(!CloudVerb::List.is_mutation());
        assert!(!CloudVerb::Inventory.is_mutation());
        assert!(!CloudVerb::Plan.is_mutation());
        assert!(CloudVerb::Provision.is_mutation());
        assert!(CloudVerb::SetDesired.is_mutation());
        assert!(CloudVerb::AndroidProvision.is_mutation());
        assert!(CloudVerb::Destroy.is_destructive());
        assert!(!CloudVerb::Provision.is_destructive());
        assert!(CloudVerb::Lifecycle(LifecycleAction::Delete).is_destructive());
        assert!(!CloudVerb::Lifecycle(LifecycleAction::Start).is_destructive());
    }

    #[test]
    fn a_request_body_parses_the_placement_and_arming_fields() {
        let b = CloudActionBody::parse(
            r#"{"node":"eagle","instance":"web","name":"web","armed_token":"tok","typed_name":"web"}"#,
        );
        assert_eq!(b.node, "eagle");
        assert_eq!(b.instance.as_deref(), Some("web"));
        assert_eq!(b.armed_token.as_deref(), Some("tok"));
        assert_eq!(b.destroy_target(), "web");
        // A malformed body degrades to all-empty (handlers then reject).
        let empty = CloudActionBody::parse("not json");
        assert!(empty.node.is_empty() && empty.armed_token.is_none());
        // destroy_target falls back to node when name is unset.
        let n = CloudActionBody::parse(r#"{"node":"eagle"}"#);
        assert_eq!(n.destroy_target(), "eagle");
    }
}
