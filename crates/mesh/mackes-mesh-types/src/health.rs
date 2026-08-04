//! Canonical System and Mesh Health contracts.
//!
//! The daemon owns evaluation and remediation authorization. Desktop surfaces
//! only render these records and submit allowlisted [`HealthActionRequest`]s.

#![allow(
    missing_docs,
    reason = "field names are the documented versioned health wire contract"
)]

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Current wire schema.
pub const HEALTH_SCHEMA_VERSION: u16 = 1;
/// Per-node state topic prefix.
pub const NODE_HEALTH_TOPIC_PREFIX: &str = "state/health/node/";
/// Roster-folded snapshot topic.
pub const SNAPSHOT_TOPIC: &str = "state/health/system-mesh";
/// Remediation request topic.
pub const ACTION_TOPIC: &str = "action/health/remediate";
/// Remediation result topic prefix.
pub const ACTION_RESULT_TOPIC_PREFIX: &str = "state/health/remediation/";
/// Critical notification lane. The chat worker remains the notification ledger.
pub const CRITICAL_NOTIFY_TOPIC: &str = "event/notify/system-mesh-health";

#[must_use]
pub fn node_health_topic(node: &str) -> String {
    format!("{NODE_HEALTH_TOPIC_PREFIX}{node}")
}

#[must_use]
pub fn action_result_topic(request_id: &str) -> String {
    format!("{ACTION_RESULT_TOPIC_PREFIX}{request_id}")
}

/// The only overall grade vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum GradeLetter {
    A,
    B,
    C,
    D,
    F,
}

impl GradeLetter {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::F => "F",
        }
    }
}

/// Typed factor breakdown. Values are capability scores, not incident severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GradeFactors {
    pub cpu: Option<u8>,
    pub memory: Option<u8>,
    pub disk: Option<u8>,
    pub system: Option<u8>,
    pub mesh: Option<u8>,
    pub devices: Option<u8>,
}

/// One node's condition-backed grade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeGrade {
    pub node: String,
    pub grade: GradeLetter,
    /// A–C capability/headroom score. D/F are selected by active conditions.
    pub capability_score: u8,
    pub factors: GradeFactors,
    pub evaluated_at_ms: u64,
}

impl NodeGrade {
    /// Grade invariant: critical => F; else warning => D; otherwise A–C only.
    #[must_use]
    pub fn evaluate(
        node: impl Into<String>,
        capability_score: u8,
        factors: GradeFactors,
        conditions: &[HealthCondition],
        evaluated_at_ms: u64,
    ) -> Self {
        let node = node.into();
        let active = conditions.iter().filter(|condition| {
            condition.is_active_for_node(&node)
                && condition.requirement == RequirementClass::Required
        });
        let mut has_warning = false;
        let mut has_critical = false;
        for condition in active {
            has_warning |= condition.severity == HealthSeverity::Warning;
            has_critical |= condition.severity == HealthSeverity::Critical;
        }
        let grade = if has_critical {
            GradeLetter::F
        } else if has_warning {
            GradeLetter::D
        } else {
            match capability_score {
                90..=u8::MAX => GradeLetter::A,
                80..=89 => GradeLetter::B,
                _ => GradeLetter::C,
            }
        };
        Self {
            node,
            grade,
            capability_score,
            factors,
            evaluated_at_ms,
        }
    }
}

/// Node or whole-mesh scope.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum HealthScope {
    Node { node: String },
    Mesh,
}

impl HealthScope {
    #[must_use]
    pub fn applies_to(&self, node: &str) -> bool {
        matches!(self, Self::Node { node: target } if target == node)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthSeverity {
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementClass {
    Required,
    Optional,
    Informational,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthComponent {
    System,
    Mesh,
    Resources,
    Devices,
    Audio,
    Firmware,
    Evidence,
}

/// Evidence is structured and timestamped so the UI never invents detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthEvidence {
    pub provider: String,
    pub summary: String,
    #[serde(default)]
    pub facts: BTreeMap<String, String>,
    pub observed_at_ms: u64,
}

/// One actionable condition with stable lifecycle identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthCondition {
    pub id: String,
    pub scope: HealthScope,
    pub component: HealthComponent,
    pub source: String,
    pub severity: HealthSeverity,
    pub requirement: RequirementClass,
    pub evidence: HealthEvidence,
    pub active_since_ms: u64,
    pub last_observed_ms: u64,
    pub resolved_at_ms: Option<u64>,
    pub acknowledged_at_ms: Option<u64>,
    pub snoozed_until_ms: Option<u64>,
    #[serde(default)]
    pub remediation: Vec<HealthRemediation>,
}

impl HealthCondition {
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.resolved_at_ms.is_none()
    }

    #[must_use]
    pub fn is_active_for_node(&self, node: &str) -> bool {
        self.is_active() && self.scope.applies_to(node)
    }

    #[must_use]
    pub fn counts_for_badge(&self, now_ms: u64) -> bool {
        self.is_active()
            && self.requirement == RequirementClass::Required
            && self.acknowledged_at_ms.is_none()
            && self.snoozed_until_ms.is_none_or(|until| until <= now_ms)
    }
}

/// An exact allowlist: arbitrary units, paths, and commands cannot be encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthAction {
    Acknowledge,
    SnoozeOneHour,
    RefreshProvider,
    RestoreWorkstationAudio,
    RefreshFirmwareMetadata,
    RestartMackesd,
    RestartMeshBus,
    RestartNebula,
    RestartSyncthing,
    RestartDns,
    RestartKdc,
    RestartShell,
    ExpandSeat15Root,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthRemediation {
    pub action: HealthAction,
    pub target: HealthScope,
    pub expected_snapshot_generation: u64,
    pub impact: String,
    pub confirmation_required: bool,
    pub workspace_route: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthActionRequest {
    pub schema_version: u16,
    pub request_id: String,
    pub condition_id: String,
    pub action: HealthAction,
    pub target: HealthScope,
    pub expected_snapshot_generation: u64,
    pub requester: String,
    pub authorization: String,
    pub confirmation: Option<String>,
    pub requested_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthActionOutcome {
    Applied,
    Refused,
    StaleGeneration,
    NotApplicable,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthActionResult {
    pub schema_version: u16,
    pub request_id: String,
    pub condition_id: String,
    pub action: HealthAction,
    pub outcome: HealthActionOutcome,
    pub detail: String,
    pub audit_id: String,
    pub completed_at_ms: u64,
    pub snapshot_generation: u64,
    pub refreshed_evidence: Option<HealthEvidence>,
}

/// Node-owned publication folded by every observer against the live roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeHealthState {
    pub schema_version: u16,
    pub publisher: String,
    pub roster_revision: String,
    pub generation: u64,
    pub published_at_ms: u64,
    pub valid_until_ms: u64,
    pub grade: NodeGrade,
    #[serde(default)]
    pub active_conditions: Vec<HealthCondition>,
    #[serde(default)]
    pub resolved_conditions: Vec<HealthCondition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshHealthSummary {
    pub grade: GradeLetter,
    pub canonical_nodes: usize,
    pub fresh_nodes: usize,
    pub reachable_lighthouses: usize,
    pub active_warnings: usize,
    pub active_critical: usize,
    pub unacknowledged_actionable: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemMeshHealthSnapshot {
    pub schema_version: u16,
    pub observer: String,
    pub roster_revision: String,
    pub generation: u64,
    pub generated_at_ms: u64,
    pub fresh_until_ms: u64,
    #[serde(default)]
    pub current_node_grades: Vec<NodeGrade>,
    #[serde(default)]
    pub active_conditions: Vec<HealthCondition>,
    #[serde(default)]
    pub resolved_conditions: Vec<HealthCondition>,
    pub mesh_summary: MeshHealthSummary,
}

impl SystemMeshHealthSnapshot {
    #[must_use]
    pub fn active_issue_count(&self, now_ms: u64) -> usize {
        self.active_conditions
            .iter()
            .filter(|condition| condition.counts_for_badge(now_ms))
            .count()
    }

    #[must_use]
    pub fn is_fresh(&self, now_ms: u64) -> bool {
        now_ms <= self.fresh_until_ms
    }
}

/// Reject non-roster, duplicate, mismatched, and expired publications.
#[must_use]
pub fn fold_snapshot(
    observer: impl Into<String>,
    roster_revision: impl Into<String>,
    canonical_nodes: &BTreeSet<String>,
    publications: Vec<NodeHealthState>,
    generation: u64,
    now_ms: u64,
    validity_ms: u64,
    reachable_lighthouses: usize,
) -> SystemMeshHealthSnapshot {
    let roster_revision = roster_revision.into();
    let mut by_publisher: BTreeMap<String, Option<NodeHealthState>> = BTreeMap::new();
    for state in publications {
        if state.schema_version != HEALTH_SCHEMA_VERSION
            || !canonical_nodes.contains(&state.publisher)
            || state.grade.node != state.publisher
            || state.roster_revision != roster_revision
            || state.published_at_ms > now_ms
            || state.valid_until_ms < now_ms
        {
            continue;
        }
        match by_publisher.entry(state.publisher.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Some(state));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.insert(None);
            }
        }
    }
    let mut accepted: Vec<_> = by_publisher.into_values().flatten().collect();
    accepted.sort_by(|left, right| left.publisher.cmp(&right.publisher));

    let mut active_conditions: Vec<_> = accepted
        .iter()
        .flat_map(|state| {
            state
                .active_conditions
                .iter()
                .filter(|condition| condition.scope.applies_to(&state.publisher))
                .cloned()
        })
        .filter(HealthCondition::is_active)
        .collect();
    let resolved_conditions = accepted
        .iter()
        .flat_map(|state| {
            state
                .resolved_conditions
                .iter()
                .filter(|condition| condition.scope.applies_to(&state.publisher))
                .cloned()
        })
        .collect();
    if accepted.len() < canonical_nodes.len() {
        let missing: Vec<_> = canonical_nodes
            .iter()
            .filter(|node| !accepted.iter().any(|state| &state.publisher == *node))
            .cloned()
            .collect();
        active_conditions.push(HealthCondition {
            id: "mesh:publisher-freshness".into(),
            scope: HealthScope::Mesh,
            component: HealthComponent::Evidence,
            source: "health-roster-fold".into(),
            severity: HealthSeverity::Warning,
            requirement: RequirementClass::Required,
            evidence: HealthEvidence {
                provider: "health-roster-fold".into(),
                summary: format!(
                    "Current health evidence is missing for: {}.",
                    missing.join(", ")
                ),
                facts: BTreeMap::from([("missing_nodes".into(), missing.join(","))]),
                observed_at_ms: now_ms,
            },
            active_since_ms: now_ms,
            last_observed_ms: now_ms,
            resolved_at_ms: None,
            acknowledged_at_ms: None,
            snoozed_until_ms: None,
            remediation: Vec::new(),
        });
    }
    let current_node_grades: Vec<NodeGrade> = accepted
        .iter()
        .map(|state| {
            let conditions: Vec<_> = active_conditions
                .iter()
                .filter(|condition| condition.scope.applies_to(&state.publisher))
                .cloned()
                .collect();
            NodeGrade::evaluate(
                state.publisher.clone(),
                state.grade.capability_score,
                state.grade.factors,
                &conditions,
                state.grade.evaluated_at_ms,
            )
        })
        .collect();
    let active_warnings = active_conditions
        .iter()
        .filter(|condition| {
            condition.requirement == RequirementClass::Required
                && condition.severity == HealthSeverity::Warning
        })
        .count();
    let active_critical = active_conditions
        .iter()
        .filter(|condition| {
            condition.requirement == RequirementClass::Required
                && condition.severity == HealthSeverity::Critical
        })
        .count();
    let unacknowledged_actionable = active_conditions
        .iter()
        .filter(|condition| condition.counts_for_badge(now_ms))
        .count();
    let grade = if active_critical > 0 {
        GradeLetter::F
    } else if active_warnings > 0 {
        GradeLetter::D
    } else {
        current_node_grades
            .iter()
            .map(|grade| grade.grade)
            .max()
            .unwrap_or(GradeLetter::C)
    };
    SystemMeshHealthSnapshot {
        schema_version: HEALTH_SCHEMA_VERSION,
        observer: observer.into(),
        roster_revision,
        generation,
        generated_at_ms: now_ms,
        fresh_until_ms: now_ms.saturating_add(validity_ms),
        current_node_grades,
        active_conditions,
        resolved_conditions,
        mesh_summary: MeshHealthSummary {
            grade,
            canonical_nodes: canonical_nodes.len(),
            fresh_nodes: accepted.len(),
            reachable_lighthouses,
            active_warnings,
            active_critical,
            unacknowledged_actionable,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn condition(node: &str, severity: HealthSeverity) -> HealthCondition {
        HealthCondition {
            id: format!("{node}:disk"),
            scope: HealthScope::Node { node: node.into() },
            component: HealthComponent::Resources,
            source: "test".into(),
            severity,
            requirement: RequirementClass::Required,
            evidence: HealthEvidence {
                provider: "fixture".into(),
                summary: "threshold breached".into(),
                facts: BTreeMap::new(),
                observed_at_ms: 100,
            },
            active_since_ms: 100,
            last_observed_ms: 100,
            resolved_at_ms: None,
            acknowledged_at_ms: None,
            snoozed_until_ms: None,
            remediation: Vec::new(),
        }
    }

    fn state(node: &str, generation: u64, now: u64) -> NodeHealthState {
        NodeHealthState {
            schema_version: HEALTH_SCHEMA_VERSION,
            publisher: node.into(),
            roster_revision: "r1".into(),
            generation,
            published_at_ms: now,
            valid_until_ms: now + 100,
            grade: NodeGrade::evaluate(node.to_string(), 95, GradeFactors::default(), &[], now),
            active_conditions: Vec::new(),
            resolved_conditions: Vec::new(),
        }
    }

    #[test]
    fn grades_below_c_require_active_conditions() {
        let c = NodeGrade::evaluate("n", 1, GradeFactors::default(), &[], 100);
        assert_eq!(c.grade, GradeLetter::C);
        let warning = condition("n", HealthSeverity::Warning);
        let d = NodeGrade::evaluate("n", 99, GradeFactors::default(), &[warning], 100);
        assert_eq!(d.grade, GradeLetter::D);
        let critical = condition("n", HealthSeverity::Critical);
        let f = NodeGrade::evaluate("n", 99, GradeFactors::default(), &[critical], 100);
        assert_eq!(f.grade, GradeLetter::F);

        let mut informational = condition("n", HealthSeverity::Critical);
        informational.requirement = RequirementClass::Informational;
        let a = NodeGrade::evaluate("n", 99, GradeFactors::default(), &[informational], 100);
        assert_eq!(
            a.grade,
            GradeLetter::A,
            "non-actionable information cannot lower the authoritative grade"
        );
    }

    #[test]
    fn badge_excludes_acknowledged_snoozed_optional_and_resolved() {
        let base = condition("n", HealthSeverity::Warning);
        assert!(base.counts_for_badge(100));
        let mut acknowledged = base.clone();
        acknowledged.acknowledged_at_ms = Some(100);
        assert!(!acknowledged.counts_for_badge(100));
        let mut snoozed = base.clone();
        snoozed.snoozed_until_ms = Some(200);
        assert!(!snoozed.counts_for_badge(100));
        let mut optional = base.clone();
        optional.requirement = RequirementClass::Optional;
        assert!(!optional.counts_for_badge(100));
        let mut resolved = base;
        resolved.resolved_at_ms = Some(100);
        assert!(!resolved.counts_for_badge(100));
    }

    #[test]
    fn fold_rejects_noncanonical_duplicate_mismatch_and_expired_rows() {
        let roster = BTreeSet::from(["a".to_string(), "b".to_string()]);
        let mut mismatch = state("b", 1, 100);
        mismatch.grade.node = "alias".into();
        let mut expired = state("b", 2, 0);
        expired.valid_until_ms = 99;
        let snapshot = fold_snapshot(
            "a",
            "r1",
            &roster,
            vec![
                state("a", 1, 100),
                state("a", 2, 100),
                state("retired", 1, 100),
                mismatch,
                expired,
            ],
            5,
            100,
            100,
            1,
        );
        assert!(
            snapshot.current_node_grades.is_empty(),
            "all duplicate rows are rejected"
        );
        assert_eq!(snapshot.mesh_summary.reachable_lighthouses, 1);
        assert_eq!(snapshot.active_conditions[0].id, "mesh:publisher-freshness");
    }
}
