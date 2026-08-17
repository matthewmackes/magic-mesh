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
        let capabilities = readiness_json
            .split("capability unavailable: ")
            .skip(1)
            .filter_map(|value| value.split(['"', '[', ']']).next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect();
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
        let view = LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string()).unwrap();
        assert_eq!(view.readiness, ReadinessState::ReadyWithWarnings);
        assert!(view.capability_summary().contains("kvm"));
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
}
