//! WL-FUNC-023 S4 — one bounded lifecycle view model for terminal and GUI clients.
//!
//! The view contains no mutation logic. Both renderers consume this projection
//! so a session cannot acquire a client-specific lifecycle interpretation.

use mackes_mesh_types::lifecycle::{
    LifecycleIntentKind, LifecyclePhase, OnboardOffboardSessionV1, SeatReadinessV1,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleSessionView {
    pub session_id: String,
    pub intent: LifecycleIntentKind,
    pub phase: LifecyclePhase,
    pub targets: Vec<String>,
    pub ready: bool,
    pub missing_requirements: Vec<String>,
    pub warnings: Vec<String>,
}

impl LifecycleSessionView {
    pub fn from_wire(session_json: &str, readiness_json: &str) -> Result<Self, String> {
        let session: OnboardOffboardSessionV1 = serde_json::from_str(session_json)
            .map_err(|_| "invalid lifecycle session".to_owned())?;
        session
            .validate()
            .map_err(|error| format!("invalid lifecycle session: {error:?}"))?;
        let readiness: SeatReadinessV1 = serde_json::from_str(readiness_json)
            .map_err(|_| "invalid lifecycle readiness".to_owned())?;
        readiness
            .validate()
            .map_err(|error| format!("invalid lifecycle readiness: {error:?}"))?;
        if !session
            .target_ids
            .iter()
            .any(|target| target == &readiness.target_id)
        {
            return Err("readiness target is outside session scope".to_owned());
        }
        Ok(Self {
            session_id: session.session_id,
            intent: session.intent,
            phase: session.phase,
            targets: session.target_ids,
            ready: readiness.ready,
            missing_requirements: readiness.missing_requirements,
            warnings: readiness.warnings,
        })
    }

    pub fn status_line(&self) -> String {
        let state = match (self.phase, self.ready) {
            (LifecyclePhase::Succeeded, true) => "ready",
            (LifecyclePhase::Failed, _) => "failed",
            (_, false) => "blocked",
            (_, true) => "in progress",
        };
        format!("{}: {} ({})", self.session_id, self.intent_label(), state)
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
}
