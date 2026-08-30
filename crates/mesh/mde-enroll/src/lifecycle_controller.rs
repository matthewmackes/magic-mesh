//! Renderer-neutral lifecycle routing for S4/S14-S16.
//!
//! This module deliberately creates plans, never performs mutations. The GUI
//! and TUI can therefore present the same offboard/reset/fleet handoff intent
//! and submit it through the authority-owned Bus path.

use mackes_mesh_types::lifecycle::{
    FleetLifecycleReportV1, LifecycleIntentKind, LifecycleIntentV1, LifecyclePhase,
    LifecyclePlanV1, LifecycleProgressV1, MAX_LIFECYCLE_IDENTIFIER_BYTES,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleController {
    pub session_id: String,
    pub generation: u64,
    pub targets: Vec<String>,
    last_progress: BTreeMap<String, LifecycleProgressV1>,
    /// Durable coordinator from authority checkpoints. Empty until first claim.
    coordinator_id: Option<String>,
}

impl LifecycleController {
    pub fn new(session_id: impl Into<String>, generation: u64, mut targets: Vec<String>) -> Self {
        targets.sort();
        targets.dedup();
        Self {
            session_id: session_id.into(),
            generation,
            targets,
            last_progress: BTreeMap::new(),
            coordinator_id: None,
        }
    }

    /// Bind a renderer to a peeked fleet report. Target count and
    /// coordinator come from durable checkpoints; a GUI cannot shrink them.
    pub fn from_fleet_report(
        report: &FleetLifecycleReportV1,
        targets: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ProgressError> {
        report
            .validate()
            .map_err(|error| ProgressError::Invalid(format!("{error:?}")))?;
        let mut controller = Self::new(
            report.request_id.clone(),
            report.generation,
            targets.into_iter().map(Into::into).collect(),
        );
        if controller.targets.is_empty() || controller.targets.len() as u32 != report.target_count {
            return Err(ProgressError::Invalid("fleet targets".into()));
        }
        controller.bind_coordinator(
            (!report.coordinator_id.is_empty()).then(|| report.coordinator_id.clone()),
        )?;
        Ok(controller)
    }

    pub fn plan(&self, intent: LifecycleIntentKind, target_id: &str) -> Option<LifecyclePlanV1> {
        if !self.targets.iter().any(|target| target == target_id) {
            return None;
        }
        // Renderers select one closed intent; the public contract owns the
        // corresponding ordered step vocabulary.  Keeping a separate GUI/TUI
        // list here previously produced terms such as `cordon` and `erase`
        // that `LifecyclePlanV1` quite correctly refused.
        let intent_request = LifecycleIntentV1 {
            schema_version: 1,
            request_id: self.session_id.clone(),
            target_id: target_id.to_owned(),
            intent,
            generation: self.generation,
        };
        Some(LifecyclePlanV1 {
            schema_version: 1,
            request_id: self.session_id.clone(),
            target_id: target_id.to_owned(),
            intent,
            generation: self.generation,
            steps: intent_request.default_steps(),
        })
    }

    pub fn fleet_targets(&self) -> &[String] {
        &self.targets
    }

    /// Bind the coordinator already recorded on durable checkpoints.
    /// A renderer cannot invent a different initiator after that.
    pub fn bind_coordinator(
        &mut self,
        coordinator_id: Option<String>,
    ) -> Result<(), ProgressError> {
        if let Some(id) = coordinator_id.as_deref() {
            if !coordinator_id_ok(id) {
                return Err(ProgressError::Invalid("coordinator".into()));
            }
        }
        self.coordinator_id = coordinator_id;
        Ok(())
    }

    /// Same coordinator line the shared session view prints.
    #[must_use]
    pub fn coordinator_line(&self) -> Option<String> {
        self.coordinator_id
            .as_ref()
            .map(|id| format!("coordinator {id}"))
    }

    /// Same fleet seat line the shared session view prints.
    #[must_use]
    pub fn fleet_line(&self) -> Option<String> {
        if self.targets.len() <= 1 {
            return None;
        }
        Some(format!("fleet {}", self.targets.join(", ")))
    }

    /// Same phrase the authority will require for this fleet intent.
    #[must_use]
    pub fn fleet_confirmation_phrase(
        &self,
        action: mackes_mesh_types::lifecycle::LifecycleConfirmationAction,
    ) -> String {
        mackes_mesh_types::lifecycle::LifecycleConfirmationV1::expected_phrase(
            action,
            self.targets.len() as u32,
        )
    }

    /// Same scope digest the authority binds to the signed phrase.
    #[must_use]
    pub fn fleet_scope_digest(&self) -> String {
        mackes_mesh_types::lifecycle::LifecycleConfirmationV1::fleet_scope_digest(&self.targets)
    }

    /// Admit a coordinator handoff for this session. The durable generation
    /// and target list stay the job; disconnecting the initiator cannot
    /// invent a replacement fleet.
    pub fn admit_fleet_handoff(
        &self,
        from_coordinator: &str,
        to_coordinator: &str,
    ) -> Result<FleetHandoffRequest, ProgressError> {
        if self.targets.is_empty() {
            return Err(ProgressError::OutOfScope);
        }
        if !coordinator_id_ok(from_coordinator) || !coordinator_id_ok(to_coordinator) {
            return Err(ProgressError::Invalid("coordinator".into()));
        }
        if from_coordinator == to_coordinator {
            return Err(ProgressError::Invalid("coordinator unchanged".into()));
        }
        if let Some(held) = self.coordinator_id.as_deref() {
            if held != from_coordinator {
                return Err(ProgressError::Invalid("coordinator mismatch".into()));
            }
        }
        Ok(FleetHandoffRequest {
            request_id: self.session_id.clone(),
            generation: self.generation,
            from_coordinator: from_coordinator.to_owned(),
            to_coordinator: to_coordinator.to_owned(),
            targets: self.targets.clone(),
        })
    }

    /// Unsigned admission pins the artifact bytes, not the seat list.
    #[must_use]
    pub fn unsigned_scope_digest(artifact_digest_hex: &str) -> Option<String> {
        if artifact_digest_hex.len() != 64
            || !artifact_digest_hex.chars().all(|c| c.is_ascii_hexdigit())
        {
            return None;
        }
        Some(artifact_digest_hex.to_owned())
    }

    /// Accept one authority progress acknowledgement for a target.
    ///
    /// Progress is a checkpoint acknowledgement, not a renderer command. The
    /// controller only accepts monotonic, target- and generation-bound
    /// checkpoints, so a stale/replayed event cannot move a renderer backward
    /// or make an interrupted session appear complete.
    pub fn acknowledge_progress(
        &mut self,
        progress: LifecycleProgressV1,
    ) -> Result<(), ProgressError> {
        progress
            .validate()
            .map_err(|error| ProgressError::Invalid(format!("{error:?}")))?;
        if progress.request_id != self.session_id {
            return Err(ProgressError::WrongSession);
        }
        if progress.generation != self.generation {
            return Err(ProgressError::StaleGeneration);
        }
        if !self
            .targets
            .iter()
            .any(|target| target == &progress.target_id)
        {
            return Err(ProgressError::OutOfScope);
        }

        if let Some(previous) = self.last_progress.get(&progress.target_id) {
            if progress.completed_steps < previous.completed_steps {
                return Err(ProgressError::Regression);
            }
            if progress.completed_steps == previous.completed_steps
                && progress.phase == previous.phase
            {
                return Err(ProgressError::Replay);
            }
            if is_terminal(previous.phase) {
                return Err(ProgressError::TerminalCheckpoint);
            }
            if progress.total_steps != previous.total_steps {
                return Err(ProgressError::TotalChanged);
            }
        }
        self.last_progress
            .insert(progress.target_id.clone(), progress);
        Ok(())
    }

    /// Return the last durable checkpoint that can be used after interruption.
    #[must_use]
    pub fn resume_checkpoint(&self, target_id: &str) -> Option<&LifecycleProgressV1> {
        self.last_progress
            .get(target_id)
            .filter(|progress| !is_terminal(progress.phase))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetHandoffRequest {
    pub request_id: String,
    pub generation: u64,
    pub from_coordinator: String,
    pub to_coordinator: String,
    pub targets: Vec<String>,
}

fn coordinator_id_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_LIFECYCLE_IDENTIFIER_BYTES
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressError {
    Invalid(String),
    WrongSession,
    StaleGeneration,
    OutOfScope,
    Regression,
    Replay,
    TerminalCheckpoint,
    TotalChanged,
}

fn is_terminal(phase: LifecyclePhase) -> bool {
    matches!(
        phase,
        LifecyclePhase::Succeeded | LifecyclePhase::Failed | LifecyclePhase::Cancelled
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_are_identical_for_gui_and_tui_consumers() {
        let mut controller =
            LifecycleController::new("fleet-1", 7, vec!["b".into(), "a".into(), "a".into()]);
        let gui = controller.plan(LifecycleIntentKind::Offboard, "a").unwrap();
        let tui = controller.plan(LifecycleIntentKind::Offboard, "a").unwrap();
        assert_eq!(gui, tui);
        assert_eq!(gui.steps, vec!["offboard", "verify"]);
        assert!(gui.validate().is_ok());
        let reset = controller
            .plan(LifecycleIntentKind::ResetAndOnboard, "a")
            .unwrap();
        assert_eq!(reset.steps.first(), Some(&"offboard".to_owned()));
        assert!(reset.validate().is_ok());
        assert!(controller
            .plan(LifecycleIntentKind::ResetAndOnboard, "missing")
            .is_none());
        assert_eq!(
            controller.fleet_confirmation_phrase(
                mackes_mesh_types::lifecycle::LifecycleConfirmationAction::Offboard
            ),
            "FORCE OFFBOARD 2 SYSTEMS"
        );
        assert_eq!(
            controller.fleet_scope_digest(),
            mackes_mesh_types::lifecycle::LifecycleConfirmationV1::fleet_scope_digest(&["a", "b"])
        );
        let digest = "e".repeat(64);
        assert_eq!(
            controller.fleet_confirmation_phrase(
                mackes_mesh_types::lifecycle::LifecycleConfirmationAction::InstallUnsigned
            ),
            "INSTALL UNSIGNED 2 SYSTEMS"
        );
        assert_eq!(
            LifecycleController::unsigned_scope_digest(&digest).as_deref(),
            Some(digest.as_str())
        );
        assert_ne!(
            LifecycleController::unsigned_scope_digest(&digest).unwrap(),
            controller.fleet_scope_digest()
        );
        assert!(LifecycleController::unsigned_scope_digest("not-a-digest").is_none());
        let handoff = controller
            .admit_fleet_handoff("coord-a", "coord-b")
            .unwrap();
        assert_eq!(handoff.request_id, "fleet-1");
        assert_eq!(handoff.generation, 7);
        assert_eq!(handoff.targets, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(
            controller.admit_fleet_handoff("coord-a", "coord-a"),
            Err(ProgressError::Invalid("coordinator unchanged".into()))
        );
        assert_eq!(
            controller.admit_fleet_handoff("coord a", "coord-b"),
            Err(ProgressError::Invalid("coordinator".into()))
        );
        controller.bind_coordinator(Some("coord-b".into())).unwrap();
        assert_eq!(
            controller.admit_fleet_handoff("coord-forged", "coord-c"),
            Err(ProgressError::Invalid("coordinator mismatch".into()))
        );
        assert_eq!(
            controller
                .admit_fleet_handoff("coord-b", "coord-c")
                .unwrap()
                .to_coordinator,
            "coord-c"
        );
    }

    #[test]
    fn from_fleet_report_binds_durable_scope_and_coordinator() {
        let report = FleetLifecycleReportV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            generation: 1,
            phase: LifecyclePhase::Running,
            target_count: 2,
            succeeded: 0,
            failed: 0,
            coordinator_id: "coord-b".into(),
            signature_hex: String::new(),
        };
        let controller =
            LifecycleController::from_fleet_report(&report, ["seat-16", "seat-15"]).unwrap();
        assert_eq!(
            controller.fleet_confirmation_phrase(
                mackes_mesh_types::lifecycle::LifecycleConfirmationAction::Offboard
            ),
            "FORCE OFFBOARD 2 SYSTEMS"
        );
        assert_eq!(
            controller.coordinator_line().as_deref(),
            Some("coordinator coord-b")
        );
        assert_eq!(
            controller.fleet_line().as_deref(),
            Some("fleet seat-15, seat-16")
        );
        assert_eq!(
            controller.admit_fleet_handoff("coord-forged", "coord-c"),
            Err(ProgressError::Invalid("coordinator mismatch".into()))
        );
        assert!(
            LifecycleController::from_fleet_report(&report, ["seat-15"]).is_err(),
            "a renderer cannot shrink a durable fleet"
        );
    }

    fn progress(
        phase: LifecyclePhase,
        completed_steps: u32,
        target_id: &str,
    ) -> LifecycleProgressV1 {
        LifecycleProgressV1 {
            schema_version: 1,
            request_id: "fleet-1".into(),
            target_id: target_id.into(),
            generation: 7,
            phase,
            completed_steps,
            total_steps: 2,
        }
    }

    #[test]
    fn accepts_typed_ack_and_returns_it_for_interruption_resume() {
        let mut controller = LifecycleController::new("fleet-1", 7, vec!["seat-15".into()]);
        controller
            .acknowledge_progress(progress(LifecyclePhase::Running, 1, "seat-15"))
            .unwrap();
        assert_eq!(
            controller
                .resume_checkpoint("seat-15")
                .map(|item| item.completed_steps),
            Some(1)
        );
    }

    #[test]
    fn rejects_replayed_stale_and_out_of_scope_acknowledgements() {
        let mut controller = LifecycleController::new("fleet-1", 7, vec!["seat-15".into()]);
        let running = progress(LifecyclePhase::Running, 1, "seat-15");
        controller.acknowledge_progress(running.clone()).unwrap();
        assert_eq!(
            controller.acknowledge_progress(running),
            Err(ProgressError::Replay)
        );
        assert_eq!(
            controller.acknowledge_progress(LifecycleProgressV1 {
                generation: 6,
                ..progress(LifecyclePhase::Running, 2, "seat-15")
            }),
            Err(ProgressError::StaleGeneration)
        );
        assert_eq!(
            controller.acknowledge_progress(progress(LifecyclePhase::Running, 2, "other")),
            Err(ProgressError::OutOfScope)
        );
    }

    #[test]
    fn rejects_regression_and_does_not_resume_after_terminal_ack() {
        let mut controller = LifecycleController::new("fleet-1", 7, vec!["seat-15".into()]);
        controller
            .acknowledge_progress(progress(LifecyclePhase::Running, 1, "seat-15"))
            .unwrap();
        assert_eq!(
            controller.acknowledge_progress(progress(LifecyclePhase::Running, 0, "seat-15")),
            Err(ProgressError::Regression)
        );
        controller
            .acknowledge_progress(progress(LifecyclePhase::Succeeded, 2, "seat-15"))
            .unwrap();
        assert!(controller.resume_checkpoint("seat-15").is_none());
        assert_eq!(
            controller.acknowledge_progress(progress(LifecyclePhase::Running, 2, "seat-15")),
            Err(ProgressError::TerminalCheckpoint)
        );
    }

    #[test]
    fn tracks_interrupted_fleet_targets_independently() {
        let mut controller =
            LifecycleController::new("fleet-1", 7, vec!["seat-15".into(), "seat-16".into()]);
        controller
            .acknowledge_progress(progress(LifecyclePhase::Running, 2, "seat-15"))
            .unwrap();
        controller
            .acknowledge_progress(progress(LifecyclePhase::Running, 0, "seat-16"))
            .unwrap();
        assert_eq!(
            controller
                .resume_checkpoint("seat-15")
                .map(|item| item.completed_steps),
            Some(2)
        );
        assert_eq!(
            controller
                .resume_checkpoint("seat-16")
                .map(|item| item.completed_steps),
            Some(0)
        );
    }
}
