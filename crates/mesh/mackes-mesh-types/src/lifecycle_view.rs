//! WL-FUNC-023 S4 — one bounded lifecycle view model for terminal and GUI clients.
//!
//! The view contains no mutation logic. Both renderers consume this projection
//! so a session cannot acquire a client-specific lifecycle interpretation.

use crate::lifecycle::{
    LifecycleIntentKind, LifecyclePhase, LifecyclePlanV1, LifecycleProgressV1,
    LifecycleRequirementCheckV1, OnboardOffboardSessionV1, SeatReadinessV1,
};

/// Warnings that begin with this prefix name a withdrawn capability (S13).
/// The projection derives the capability name from the *typed* warning so a
/// renderer never re-parses the raw readiness envelope.
const CAPABILITY_UNAVAILABLE_PREFIX: &str = "capability unavailable: ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleSessionView {
    pub session_id: String,
    pub intent: LifecycleIntentKind,
    pub phase: LifecyclePhase,
    pub targets: Vec<String>,
    pub ready: bool,
    pub missing_requirements: Vec<String>,
    pub warnings: Vec<String>,
    pub readiness: ReadinessState,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessState {
    Blocked,
    ReadyWithWarnings,
    Ready,
}

impl ReadinessState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::ReadyWithWarnings => "ready with warnings",
            Self::Ready => "ready",
        }
    }
}

impl LifecycleSessionView {
    /// Derive withdrawn-capability names from the typed readiness warnings.
    ///
    /// Only a warning whose text *starts with* the capability prefix names a
    /// capability; a warning that merely mentions the phrase mid-sentence is
    /// left as an ordinary warning. The trailing name is trimmed and empties
    /// are dropped so a renderer receives a clean capability list.
    fn capabilities_from_warnings(warnings: &[String]) -> Vec<String> {
        warnings
            .iter()
            .filter_map(|warning| warning.strip_prefix(CAPABILITY_UNAVAILABLE_PREFIX))
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect()
    }

    pub fn from_wire(session_json: &str, readiness_json: &str) -> Result<Self, String> {
        let session: OnboardOffboardSessionV1 = serde_json::from_str(session_json)
            .map_err(|_| "invalid lifecycle session".to_owned())?;
        session
            .validate()
            .map_err(|error| format!("invalid lifecycle session: {error:?}"))?;
        let readiness_wire: SeatReadinessV1 = serde_json::from_str(readiness_json)
            .map_err(|_| "invalid lifecycle readiness".to_owned())?;
        readiness_wire
            .validate()
            .map_err(|error| format!("invalid lifecycle readiness: {error:?}"))?;
        if !session
            .target_ids
            .iter()
            .any(|target| target == &readiness_wire.target_id)
        {
            return Err("readiness target is outside session scope".to_owned());
        }
        if readiness_wire.generation != session.generation {
            return Err("readiness generation does not match session".to_owned());
        }
        let readiness = if !readiness_wire.missing_requirements.is_empty() {
            ReadinessState::Blocked
        } else if !readiness_wire.warnings.is_empty() {
            ReadinessState::ReadyWithWarnings
        } else {
            ReadinessState::Ready
        };
        let capabilities = Self::capabilities_from_warnings(&readiness_wire.warnings);
        Ok(Self {
            session_id: session.session_id,
            intent: session.intent,
            phase: session.phase,
            targets: session.target_ids,
            ready: readiness_wire.ready,
            missing_requirements: readiness_wire.missing_requirements,
            warnings: readiness_wire.warnings,
            readiness,
            capabilities,
        })
    }

    /// Project an authority checkpoint through the same session/readiness
    /// wire the GUI and TUI already share. `operator_id` names the local
    /// authority, not a dest or a signed operator.
    pub fn from_authority_parts(
        plan: &LifecyclePlanV1,
        progress: &LifecycleProgressV1,
        checks: &[LifecycleRequirementCheckV1],
    ) -> Result<Self, String> {
        let readiness = SeatReadinessV1::from_requirement_checks(
            plan.schema_version,
            plan.target_id.clone(),
            plan.generation,
            checks,
        )
        .map_err(|error| format!("invalid lifecycle readiness: {error:?}"))?;
        let session = OnboardOffboardSessionV1 {
            schema_version: plan.schema_version,
            session_id: plan.request_id.clone(),
            operator_id: "local-authority".to_owned(),
            intent: plan.intent,
            target_ids: vec![plan.target_id.clone()],
            generation: plan.generation,
            phase: progress.phase,
        };
        session
            .validate()
            .map_err(|error| format!("invalid lifecycle session: {error:?}"))?;
        let session_json =
            serde_json::to_string(&session).map_err(|_| "invalid lifecycle session".to_owned())?;
        let readiness_json = serde_json::to_string(&readiness)
            .map_err(|_| "invalid lifecycle readiness".to_owned())?;
        Self::from_wire(&session_json, &readiness_json)
    }

    pub fn status_line(&self) -> String {
        let state = match self.phase {
            LifecyclePhase::Failed => "failed",
            LifecyclePhase::Succeeded => self.readiness.label(),
            _ if self.readiness == ReadinessState::Blocked => "blocked",
            _ => "in progress",
        };
        format!("{}: {} ({})", self.session_id, self.intent_label(), state)
    }

    pub fn capability_summary(&self) -> String {
        if self.capabilities.is_empty() {
            "capabilities: baseline available".to_owned()
        } else {
            format!("capabilities unavailable: {}", self.capabilities.join(", "))
        }
    }

    fn intent_label(&self) -> &'static str {
        match self.intent {
            LifecycleIntentKind::Onboard => "onboard",
            LifecycleIntentKind::Upgrade => "upgrade",
            LifecycleIntentKind::VerifyAndCorrect => "verify-and-correct",
            LifecycleIntentKind::Offboard => "offboard",
            LifecycleIntentKind::ResetAndOnboard => "reset-and-onboard",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_a_scoped_session_for_any_renderer() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "onboard", "target_ids": ["seat-15"], "generation": 1, "phase": "succeeded"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [], "warnings": []
        });
        let view =
            LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string()).unwrap();
        assert_eq!(view.status_line(), "session-1: onboard (ready)");
        assert_eq!(view.readiness, ReadinessState::Ready);
    }

    #[test]
    fn projects_ready_with_warnings_without_blocking_core_readiness() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "verify_and_correct", "target_ids": ["seat-15"], "generation": 1, "phase": "succeeded"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [], "warnings": ["capability unavailable: kvm"]
        });
        let view =
            LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string()).unwrap();
        assert_eq!(view.readiness, ReadinessState::ReadyWithWarnings);
        assert!(view.capability_summary().contains("kvm"));
    }

    #[test]
    fn capabilities_are_projected_from_typed_warnings_not_raw_json() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "verify_and_correct", "target_ids": ["seat-15"], "generation": 1, "phase": "running"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [],
            "warnings": ["capability unavailable: kvm", "capability unavailable: gpu passthrough"]
        });
        let session_json = session.to_string();
        let compact = serde_json::to_string(&readiness).unwrap();
        let pretty = serde_json::to_string_pretty(&readiness).unwrap();
        assert_ne!(
            compact, pretty,
            "fixture must actually differ in wire layout"
        );
        let compact_view = LifecycleSessionView::from_wire(&session_json, &compact).unwrap();
        let pretty_view = LifecycleSessionView::from_wire(&session_json, &pretty).unwrap();
        assert_eq!(compact_view.capabilities, pretty_view.capabilities);
        assert_eq!(pretty_view.capabilities, vec!["kvm", "gpu passthrough"]);
        assert_eq!(
            pretty_view.capability_summary(),
            "capabilities unavailable: kvm, gpu passthrough"
        );
    }

    #[test]
    fn does_not_harvest_a_capability_from_a_mid_sentence_mention() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "onboard", "target_ids": ["seat-15"], "generation": 1, "phase": "running"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [],
            "warnings": ["note: this is not a capability unavailable: report"]
        });
        let view =
            LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string()).unwrap();
        assert!(view.capabilities.is_empty());
        assert_eq!(
            view.capability_summary(),
            "capabilities: baseline available"
        );
        assert_eq!(view.readiness, ReadinessState::ReadyWithWarnings);
    }

    #[test]
    fn drops_empty_capability_names_after_the_typed_prefix() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "onboard", "target_ids": ["seat-15"], "generation": 1, "phase": "running"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [],
            "warnings": [
                "capability unavailable: ",
                "capability unavailable:    ",
                "capability unavailable: kvm"
            ]
        });
        let view =
            LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string()).unwrap();
        assert_eq!(view.capabilities, vec!["kvm"]);
        assert_eq!(view.capability_summary(), "capabilities unavailable: kvm");
    }

    #[test]
    fn rejects_readiness_outside_session_scope() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "offboard", "target_ids": ["seat-15"], "generation": 1, "phase": "running"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-16", "generation": 1,
            "ready": false, "missing_requirements": ["mesh_identity"], "warnings": []
        });
        assert!(
            LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string()).is_err()
        );
    }

    #[test]
    fn rejects_stale_readiness_generation() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "upgrade", "target_ids": ["seat-15"], "generation": 2, "phase": "running"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [], "warnings": []
        });
        let error = LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string())
            .expect_err("stale readiness must not project into a newer session");
        assert_eq!(error, "readiness generation does not match session");
    }

    #[test]
    fn from_authority_parts_uses_shared_readiness_not_a_fabricated_ready() {
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
            steps: vec!["identity".into()],
        };
        let progress = LifecycleProgressV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            generation: 1,
            phase: LifecyclePhase::WaitingForOperator,
            completed_steps: 0,
            total_steps: 1,
        };
        let checks = vec![LifecycleRequirementCheckV1 {
            schema_version: 1,
            check_id: "identity".into(),
            target_id: "seat-15".into(),
            expected: "present".into(),
            observed: "missing".into(),
            status: crate::lifecycle::LifecycleCheckStatus::Fail,
            required: true,
            evidence_digest_hex: "2".repeat(64),
            warning: None,
            generation: 1,
        }];
        let view = LifecycleSessionView::from_authority_parts(&plan, &progress, &checks).unwrap();
        assert_eq!(view.status_line(), "request-1: onboard (blocked)");
        assert_eq!(view.missing_requirements, vec!["identity"]);
        let ready = LifecycleSessionView::from_authority_parts(&plan, &progress, &[]).unwrap();
        assert!(ready.missing_requirements.is_empty());
        assert_eq!(ready.status_line(), "request-1: onboard (in progress)");
    }
}
